// app.rs — Central application state

use crate::db::{Database, Game};
use anyhow::Result;

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
    Done    { app_id: u32, success: bool },
}

// ─────────────────────────────────────────────
// Wishlist entry (populated from API)
// ─────────────────────────────────────────────
#[derive(Debug, Clone)]
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

    // Install/uninstall
    pub install_state: InstallState,

    // Wishlist tab
    pub wishlist:      Vec<WishlistEntry>,
    pub wishlist_sel:  usize,
    pub wishlist_sort: WishlistSort,
    pub wishlist_loading: bool,

    // Stats tab — derived lazily
    pub stats_dirty:   bool,

    // Global
    pub status_msg:    Option<String>,
    pub should_quit:   bool,
    pub needs_clear:   bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WishlistSort {
    Deal,
    Discount,
    Price,
}

impl App {
    pub fn new(db: &Database) -> Result<Self> {
        let games = db.list_games()?;
        let count = games.len();

        let mut app = Self {
            active_tab:       Tab::Library,
            games,
            filtered:         (0..count).collect(),
            selected:         0,
            search:           String::new(),
            search_mode:      false,
            install_state:    InstallState::Idle,
            wishlist:         Vec::new(),
            wishlist_sel:     0,
            wishlist_sort:    WishlistSort::Deal,
            wishlist_loading: false,
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
