// image.rs — Terminal image rendering via chafa
//
// Why chafa instead of kitty icat?
// The app runs inside tmux (TERM=tmux-256color). tmux intercepts and corrupts
// the kitty graphics protocol escape sequences. chafa auto-detects the best
// protocol the terminal stack supports (sixel, unicode half-blocks, braille)
// and works correctly through tmux.
//
// Install: sudo apt install chafa  OR  brew install chafa

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub struct ImageRenderer {
    pub available: bool,
    cache_dir: PathBuf,
    current: std::cell::Cell<Option<u32>>,
}

impl ImageRenderer {
    pub fn new() -> Self {
        let available = chafa_available();
        let cache_dir = cache_dir();
        if available {
            let _ = std::fs::create_dir_all(&cache_dir);
        }
        Self {
            available,
            cache_dir,
            current: std::cell::Cell::new(None),
        }
    }

    /// Render the game header image into the terminal at the given position.
    /// `col` / `row`    : 1-based terminal cell of the top-left corner
    /// `width` / `height` : size in terminal cells
    ///
    /// Only re-renders when the appid changes to avoid flicker.
    pub fn render(&self, app_id: u32, col: u16, row: u16, width: u16, height: u16) {
        if !self.available || width == 0 || height == 0 {
            return;
        }

        let path = self.cache_dir.join(format!("{}.jpg", app_id));
        if !path.exists() {
            if fetch_header(app_id, &path).is_err() {
                return;
            }
        }

        if self.current.get() == Some(app_id) {
            return;
        }
        self.current.set(Some(app_id));

        render_chafa(&path, col, row, width, height);
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

fn render_chafa(path: &Path, col: u16, row: u16, width: u16, height: u16) {
    // Run chafa to produce the escape-coded image output
    let output = Command::new("chafa")
        .args([
            "--size",    &format!("{}x{}", width, height),
            "--align",   "left",
            "--animate", "false",
            "--stretch",
            path.to_str().unwrap_or(""),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();

    let output = match output {
        Ok(o) => o,
        Err(_) => return,
    };

    // Write chafa output line by line, moving the cursor to the correct
    // column before each line so the image lands inside the detail panel.
    let mut stdout = std::io::stdout().lock();
    let text = String::from_utf8_lossy(&output.stdout);

    for (i, line) in text.lines().enumerate() {
        // ANSI: move cursor to absolute position (row+i, col)
        let _ = write!(stdout, "\x1b[{};{}H{}", row as usize + i, col, line);
    }

    // Return cursor to top-left so ratatui isn't confused
    let _ = write!(stdout, "\x1b[1;1H");
    let _ = stdout.flush();
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

fn fetch_header(app_id: u32, dest: &Path) -> anyhow::Result<()> {
    let url = format!(
        "https://cdn.cloudflare.steamstatic.com/steam/apps/{}/header.jpg",
        app_id
    );
    let bytes = reqwest::blocking::get(url)?.bytes()?;
    std::fs::write(dest, &bytes)?;
    Ok(())
}
