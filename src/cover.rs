// image.rs — Cover art: download/cache plus in-layout rendering via ratatui-image
//
// ratatui-image picks the best protocol the terminal supports (kitty, sixel,
// iTerm2, or unicode half-blocks) and handles tmux passthrough, so the cover
// is an ordinary widget drawn inside the ratatui layout.

use ratatui::{layout::Rect, Frame};
use ratatui_image::{picker::Picker, protocol::StatefulProtocol, StatefulImage};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub struct ImageRenderer {
    picker:    Picker,
    cache_dir: PathBuf,
    /// AppID and protocol of the cover currently on screen.
    current:   Option<(u32, StatefulProtocol)>,
    client:    reqwest::blocking::Client,
    /// AppIDs we've already determined have no usable artwork. Avoids
    /// hammering the CDN every frame for genuinely-missing games.
    failed:    Arc<Mutex<HashSet<u32>>>,
    /// AppIDs currently being downloaded in a background worker thread.
    in_flight: Arc<Mutex<HashSet<u32>>>,
}

impl ImageRenderer {
    pub fn new(client: reqwest::blocking::Client, picker: Picker) -> Self {
        let cache_dir = cache_dir();
        let _ = std::fs::create_dir_all(&cache_dir);
        Self {
            picker,
            cache_dir,
            current:   None,
            client,
            failed:    Arc::new(Mutex::new(HashSet::new())),
            in_flight: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Draw the game header image into `area`.
    ///
    /// If the image isn't cached yet, kicks off a background fetch and draws
    /// nothing; a later frame picks the file up once it exists.
    pub fn render(&mut self, f: &mut Frame, app_id: u32, area: Rect) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        if self.current.as_ref().map(|(id, _)| *id) != Some(app_id) {
            self.current = None;
            let path = self.cache_dir.join(format!("{}.jpg", app_id));
            if !path.exists() {
                if !self.failed.lock().map(|f| f.contains(&app_id)).unwrap_or(false) {
                    self.spawn_fetch(app_id, path);
                }
                return;
            }
            match image::open(&path) {
                Ok(img) => self.current = Some((app_id, self.picker.new_resize_protocol(img))),
                Err(_) => {
                    // Corrupt cache entry: drop it so a later pass can refetch.
                    let _ = std::fs::remove_file(&path);
                    return;
                }
            }
        }

        if let Some((_, protocol)) = self.current.as_mut() {
            f.render_stateful_widget(StatefulImage::default(), area, protocol);
        }
    }

    fn spawn_fetch(&self, app_id: u32, dest: PathBuf) {
        {
            let mut g = match self.in_flight.lock() { Ok(g) => g, Err(_) => return };
            if !g.insert(app_id) { return; } // already fetching
        }
        let client     = self.client.clone();
        let failed     = Arc::clone(&self.failed);
        let in_flight  = Arc::clone(&self.in_flight);
        std::thread::spawn(move || {
            let ok = fetch_header(&client, app_id, &dest).is_ok();
            if !ok {
                if let Ok(mut f) = failed.lock() { f.insert(app_id); }
            }
            if let Ok(mut g) = in_flight.lock() { g.remove(&app_id); }
        });
    }
}

// ─────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────

fn cache_dir() -> PathBuf {
    let base = std::env::var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".cache")
        });
    base.join("noguisteam").join("headers")
}

fn fetch_header(client: &reqwest::blocking::Client, app_id: u32, dest: &Path) -> anyhow::Result<()> {
    // Try a chain of CDN variants. Some apps lack header.jpg but have
    // capsule/library art. First validated JPEG wins.
    const VARIANTS: &[&str] = &[
        "header.jpg",
        "capsule_616x353.jpg",
        "library_600x900.jpg",
        "library_hero.jpg",
        "capsule_231x87.jpg",
    ];

    let mut last_err: Option<anyhow::Error> = None;
    for variant in VARIANTS {
        let url = format!(
            "https://cdn.cloudflare.steamstatic.com/steam/apps/{}/{}",
            app_id, variant
        );
        match try_fetch(client, &url, dest) {
            Ok(()) => return Ok(()),
            Err(e) => last_err = Some(e),
        }
    }

    // Newer apps keep their art under a hashed path
    // (store_item_assets/steam/apps/{id}/{hash}/header.jpg) that only the
    // store API knows; the fixed URLs above 404 for them.
    match store_header_url(client, app_id).and_then(|url| try_fetch(client, &url, dest)) {
        Ok(()) => Ok(()),
        Err(e) => Err(last_err.unwrap_or(e)),
    }
}

/// `header_image` from the store's appdetails API.
fn store_header_url(client: &reqwest::blocking::Client, app_id: u32) -> anyhow::Result<String> {
    let body: serde_json::Value = client
        .get("https://store.steampowered.com/api/appdetails")
        .query(&[("appids", app_id.to_string()), ("filters", "basic".into())])
        .send()?
        .error_for_status()?
        .json()?;
    header_image_from(&body, app_id)
        .ok_or_else(|| anyhow::anyhow!("appdetails has no header_image for {app_id}"))
}

fn header_image_from(body: &serde_json::Value, app_id: u32) -> Option<String> {
    body.get(app_id.to_string())?
        .get("data")?
        .get("header_image")?
        .as_str()
        .map(str::to_string)
}

fn try_fetch(client: &reqwest::blocking::Client, url: &str, dest: &Path) -> anyhow::Result<()> {
    let resp = client.get(url).send()?.error_for_status()?;
    let bytes = resp.bytes()?;

    // JPEG magic: FF D8 FF. Reject HTML 404 bodies and other garbage.
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 || bytes[2] != 0xFF {
        anyhow::bail!("not a JPEG ({} bytes)", bytes.len());
    }

    // Atomic write: temp file + rename so partial downloads never poison cache.
    let tmp = dest.with_extension("jpg.tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, dest)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_image_is_read_from_appdetails() {
        let body: serde_json::Value = serde_json::from_str(r#"{"4038320":{"success":true,"data":{
            "header_image":"https://shared.akamai.steamstatic.com/store_item_assets/steam/apps/4038320/5754be/header.jpg?t=1"}}}"#).unwrap();
        assert_eq!(
            header_image_from(&body, 4038320).as_deref(),
            Some("https://shared.akamai.steamstatic.com/store_item_assets/steam/apps/4038320/5754be/header.jpg?t=1"),
        );
        let missing: serde_json::Value = serde_json::from_str(r#"{"1":{"success":false}}"#).unwrap();
        assert_eq!(header_image_from(&missing, 1), None);
    }
}
