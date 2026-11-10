// app.rs — Central application state

use crate::db::{Database, Game};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ─────────────────────────────────────────────
// Tabs
// ─────────────────────────────────────────────
#[derive(Debug, Clone, PartialEq)]
pub enum Tab {
    Library,
    Wishlist,
    Stats,
}

impl Tab {
    pub fn titles() -> Vec<&'static str> {
        vec!["  Library  ", "  Wishlist  ", "  Stats  "]
    }

    pub fn index(&self) -> usize {
        match self {
            Tab::Library  => 0,
            Tab::Wishlist => 1,
            Tab::Stats    => 2,
        }
    }

    pub fn from_index(i: usize) -> Self {
        match i {
            0 => Tab::Library,
            1 => Tab::Wishlist,
            _ => Tab::Stats,
        }
    }
}

// ─────────────────────────────────────────────
// Install progress
// ─────────────────────────────────────────────
#[derive(Debug, Clone)]
pub enum InstallState {
    Idle,
    Running { app_id: u32, output: Vec<String> },
    Done    { app_id: u32, success: bool, output: Vec<String> },
}

/// Lines kept for the popup; 50GB games emit thousands.
pub const INSTALL_OUTPUT_CAP: usize = 200;

/// What an install or uninstall thread sends. Only `Finished` decides the
/// outcome: steamcmd output often says "error" or "failed" about harmless things.
#[derive(Debug, Clone)]
pub enum InstallMsg {
    Line(String),
    Finished(Result<String, String>),
}

impl InstallState {
    /// Takes one message from the install thread. Returns true when the
    /// install just succeeded, so the caller can reload the library.
    pub fn feed(&mut self, msg: InstallMsg) -> bool {
        let InstallState::Running { app_id, output } = self else { return false };
        match msg {
            InstallMsg::Line(line) => {
                if line.contains("Waiting for confirmation") {
                    if !output.iter().any(|l| l.contains("📱")) {
                        output.push("📱 Approve Steam Guard in your mobile app…".to_string());
                    }
                } else {
                    push_bounded(output, line);
                }
                false
            }
            InstallMsg::Finished(result) => {
                let (app_id, mut output) = (*app_id, std::mem::take(output));
                let success = result.is_ok();
                push_bounded(&mut output, match result {
                    Ok(m)  => format!("✅ {m}"),
                    Err(m) => format!("❌ {m}"),
                });
                *self = InstallState::Done { app_id, success, output };
                success
            }
        }
    }
}

fn push_bounded(output: &mut Vec<String>, line: String) {
    output.push(line);
    if output.len() > INSTALL_OUTPUT_CAP {
        let drop = output.len() - INSTALL_OUTPUT_CAP;
        output.drain(0..drop);
    }
}

// ─────────────────────────────────────────────
// Wishlist entry (populated from API)
// ─────────────────────────────────────────────
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WishlistEntry {
    pub name:             String,
    pub current_price:    f64,
    pub regular_price:    f64,
    pub discount_percent: u32,
    pub deal_tag:         String,
    pub store_low:        Option<f64>,
    pub historical_low:   Option<f64>,
    pub url:              String,   // Steam store page URL
}

impl WishlistEntry {
    /// Money saved (regular − current) weighted by the discount fraction, so
    /// a large saving at a deep discount ranks above the same saving on a
    /// small markdown of an expensive game.
    pub fn deal_score(&self) -> f64 {
        let saved = (self.regular_price - self.current_price).max(0.0);
        saved * f64::from(self.discount_percent) / 100.0
    }
}

// ─────────────────────────────────────────────
// App state
// ─────────────────────────────────────────────
pub struct App {
    // Navigation
    pub active_tab:    Tab,

    // Library tab
    pub games:         Vec<Game>,
    pub filtered:      Vec<usize>,   // indices into games
    pub selected:      usize,        // index into filtered
    pub search:        String,
    pub search_mode:   bool,
    /// `?` help popup listing every key is open.
    pub show_help:     bool,

    // Install/uninstall
    pub install_state: InstallState,

    // Wishlist tab
    pub wishlist:      Vec<WishlistEntry>,
    pub wishlist_sel:  usize,
    pub wishlist_sort: WishlistSort,
    pub wishlist_loading: bool,

    // Library sync
    pub sync_loading:  bool,

    // Steam library path — used by UI to look up on-disk sizes from appmanifests
    pub steamlib:      PathBuf,

    // Stats tab — derived lazily
    pub stats_dirty:   bool,

    // Global
    pub status_msg:    Option<String>,
    pub should_quit:   bool,
    pub needs_clear:   bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WishlistSort {
    /// Biggest saving weighted by discount: see `WishlistEntry::deal_score`.
    Deal,
    Discount,
    Price,
}

impl App {
    pub fn new(db: &Database, steamlib: PathBuf) -> Result<Self> {
        let games = db.list_games()?;
        let count = games.len();

        let mut app = Self {
            active_tab:       Tab::Library,
            games,
            filtered:         (0..count).collect(),
            selected:         0,
            search:           String::new(),
            search_mode:      false,
            show_help:        false,
            install_state:    InstallState::Idle,
            wishlist:         Vec::new(),
            wishlist_sel:     0,
            wishlist_sort:    WishlistSort::Deal,
            wishlist_loading: false,
            sync_loading:     false,
            steamlib,
            stats_dirty:      true,
            status_msg:       None,
            should_quit:      false,
            needs_clear:      false,
        };

        app.apply_filter();
        Ok(app)
    }

    // ── Library ──────────────────────────────

    pub fn selected_game(&self) -> Option<&Game> {
        self.filtered
            .get(self.selected)
            .and_then(|&i| self.games.get(i))
    }

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub fn move_down(&mut self) {
        if self.selected + 1 < self.filtered.len() {
            self.selected += 1;
        }
    }

    pub fn apply_filter(&mut self) {
        let q = self.search.to_lowercase();
        self.filtered = self.games
            .iter()
            .enumerate()
            .filter(|(_, g)| g.name.to_lowercase().contains(&q))
            .map(|(i, _)| i)
            .collect();

        // Keep selection in bounds
        if self.selected >= self.filtered.len() {
            self.selected = self.filtered.len().saturating_sub(1);
        }
    }

    pub fn reload_games(&mut self, db: &Database) -> Result<()> {
        self.games = db.list_games()?;
        self.apply_filter();
        self.stats_dirty = true;
        Ok(())
    }

    // ── Tabs ─────────────────────────────────

    pub fn next_tab(&mut self) {
        let next = (self.active_tab.index() + 1) % Tab::titles().len();
        self.active_tab = Tab::from_index(next);
    }

    pub fn prev_tab(&mut self) {
        let i = self.active_tab.index();
        let prev = if i == 0 { Tab::titles().len() - 1 } else { i - 1 };
        self.active_tab = Tab::from_index(prev);
    }

    // ── Status bar ───────────────────────────

    pub fn set_status(&mut self, msg: impl Into<String>) {
        self.status_msg = Some(msg.into());
    }

    pub fn clear_status(&mut self) {
        self.status_msg = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running() -> InstallState {
        InstallState::Running { app_id: 7, output: vec!["Starting install for X…".into()] }
    }

    fn line(s: &str) -> InstallMsg { InstallMsg::Line(s.into()) }

    fn lines(s: &InstallState) -> Vec<&str> {
        match s {
            InstallState::Running { output, .. } | InstallState::Done { output, .. } => {
                output.iter().map(|l| l.as_str()).collect()
            }
            InstallState::Idle => vec![],
        }
    }

    #[test]
    fn output_mentioning_failure_does_not_end_the_install() {
        let mut s = running();
        s.feed(line("Failed to init SDL priority manager: SDL not found"));
        s.feed(line("Error! App '7' state is 0x202 after update job."));
        assert!(matches!(s, InstallState::Running { .. }));
        assert_eq!(lines(&s).len(), 3);
    }

    #[test]
    fn a_failed_result_ends_it_with_the_reason_last() {
        let mut s = running();
        s.feed(line("Logging in user 'x' to Steam Public...ERROR (Invalid Password)"));
        assert!(!s.feed(InstallMsg::Finished(Err("steamcmd exited with 5 — run `noguisteam login`".into()))));
        assert!(matches!(s, InstallState::Done { success: false, .. }));
        assert!(lines(&s)[1].contains("Invalid Password"));
        assert_eq!(lines(&s).last(), Some(&"❌ steamcmd exited with 5 — run `noguisteam login`"));
    }

    #[test]
    fn a_successful_result_reports_a_reload() {
        let mut s = running();
        assert!(!s.feed(line("Update state (0x61) downloading")));
        assert!(s.feed(InstallMsg::Finished(Ok("7 installed.".into()))));
        assert!(matches!(s, InstallState::Done { success: true, .. }));
        assert_eq!(lines(&s).last(), Some(&"✅ 7 installed."));
    }

    #[test]
    fn steam_guard_waiting_is_shown_once() {
        let mut s = running();
        s.feed(line("Waiting for confirmation..."));
        s.feed(line("Waiting for confirmation..."));
        assert_eq!(lines(&s).iter().filter(|l| l.contains("📱")).count(), 1);
    }

    #[test]
    fn output_is_bounded() {
        let mut s = running();
        for i in 0..500 { s.feed(InstallMsg::Line(format!("progress {i}"))); }
        assert_eq!(lines(&s).len(), INSTALL_OUTPUT_CAP);
        assert_eq!(lines(&s).last(), Some(&"progress 499"));
    }
}
