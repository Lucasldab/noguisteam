// image.rs — Terminal image rendering via chafa
//
// Why chafa instead of kitty icat?
// The app runs inside tmux (TERM=tmux-256color). tmux intercepts and corrupts
// the kitty graphics protocol escape sequences. chafa auto-detects the best
// protocol the terminal stack supports (sixel, unicode half-blocks, braille)
// and works correctly through tmux.
//
// Install: sudo apt install chafa  OR  brew install chafa

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

pub struct ImageRenderer {
    pub available: bool,
    cache_dir: PathBuf,
    current:   std::cell::Cell<Option<u32>>,
    client:    reqwest::blocking::Client,
    /// AppIDs we've already determined have no usable artwork. Avoids
    /// hammering the CDN every frame for genuinely-missing games.
    failed:    Arc<Mutex<HashSet<u32>>>,
    /// AppIDs currently being downloaded in a background worker thread.
    in_flight: Arc<Mutex<HashSet<u32>>>,
    /// Worker → render-loop notification channel. Each `()` means a fetch
    /// finished — the caller checks `path.exists()` and re-renders.
    done_tx:   mpsc::Sender<()>,
    pub done_rx: mpsc::Receiver<()>,
}

impl ImageRenderer {
    pub fn new(client: reqwest::blocking::Client) -> Self {
        let available = chafa_available();
        let cache_dir = cache_dir();
        if available {
            let _ = std::fs::create_dir_all(&cache_dir);
        }
        let (done_tx, done_rx) = mpsc::channel();
        Self {
            available,
            cache_dir,
            current:   std::cell::Cell::new(None),
            client,
            failed:    Arc::new(Mutex::new(HashSet::new())),
            in_flight: Arc::new(Mutex::new(HashSet::new())),
            done_tx,
            done_rx,
        }
    }

    /// Render the game header image into the terminal at the given position.
    /// `col` / `row`    : 1-based terminal cell of the top-left corner
    /// `width` / `height` : size in terminal cells
    ///
    /// If the image isn't cached yet, kicks off a background fetch and returns
    /// immediately (the worker sends `()` on `done_rx` when ready, prompting
    /// the run loop to retry).
    pub fn render(&self, app_id: u32, col: u16, row: u16, width: u16, height: u16) {
        if !self.available || width == 0 || height == 0 {
            return;
        }

        let path = self.cache_dir.join(format!("{}.jpg", app_id));
        if !path.exists() {
            if self.failed.lock().map(|f| f.contains(&app_id)).unwrap_or(false) {
                return;
            }
            self.spawn_fetch(app_id, path);
            return;
        }

        if self.current.get() == Some(app_id) {
            return;
        }

        if render_chafa(&path, col, row, width, height) {
            self.current.set(Some(app_id));
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
        let done_tx    = self.done_tx.clone();
        std::thread::spawn(move || {
            let ok = fetch_header(&client, app_id, &dest).is_ok();
            if !ok {
                if let Ok(mut f) = failed.lock() { f.insert(app_id); }
            }
            if let Ok(mut g) = in_flight.lock() { g.remove(&app_id); }
            let _ = done_tx.send(());
        });
    }

    /// Erase the image area by overwriting it with blank lines.
    pub fn clear(&self) {
        if !self.available { return; }
        self.current.set(None);
    }
}

// ─────────────────────────────────────────────
// chafa rendering
// ─────────────────────────────────────────────

fn render_chafa(path: &Path, col: u16, row: u16, width: u16, height: u16) -> bool {
    let Some(path_str) = path.to_str() else { return false; };

    let output = Command::new("chafa")
        .args([
            "--size",    &format!("{}x{}", width, height),
            "--align",   "left",
            "--animate", "false",
            "--stretch",
            path_str,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();

    let output = match output {
        Ok(o) if o.status.success() => o,
        _ => return false,
    };

    let mut stdout = std::io::stdout().lock();
    let text = String::from_utf8_lossy(&output.stdout);

    for (i, line) in text.lines().enumerate() {
        let _ = write!(stdout, "\x1b[{};{}H{}", row as usize + i, col, line);
    }

    let _ = write!(stdout, "\x1b[1;1H");
    let _ = stdout.flush();
    true
}

// ─────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────

fn chafa_available() -> bool {
    Command::new("chafa")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

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
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("no variants fetched")))
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
