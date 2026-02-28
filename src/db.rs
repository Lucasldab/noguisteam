// db.rs — SQLite access via rusqlite

use anyhow::Result;
use rusqlite::{Connection, params};
use std::path::Path;

// ─────────────────────────────────────────────
// Game row
// ─────────────────────────────────────────────
#[derive(Debug, Clone)]
pub struct Game {
    pub app_id:          u32,
    pub name:            String,
    pub playtime_mins:   u32,
    pub last_played:     u64,   // unix timestamp, 0 = never
    pub installed:       bool,
}

#[allow(dead_code)]
impl Game {
    pub fn playtime_fmt(&self) -> String {
        let h = self.playtime_mins / 60;
        let m = self.playtime_mins % 60;
        if h == 0 {
            format!("{}m", m)
        } else {
            format!("{}h {}m", h, m)
        }
    }

    pub fn last_played_fmt(&self) -> String {
        if self.last_played == 0 {
            return "Never".to_string();
        }
        // Simple formatting without chrono dependency
        let secs  = self.last_played;
        let days  = secs / 86400;
        let epoch_days_to_2000: u64 = 10957; // days from 1970 to 2000
        if days < epoch_days_to_2000 {
            return "Never".to_string();
        }
        // Return raw timestamp — ui.rs can format further if needed
        format!("ts:{}", secs)
    }
}

// ─────────────────────────────────────────────
// Database handle
// ─────────────────────────────────────────────
pub struct Database {
    conn: Connection,
}

#[allow(dead_code)]
impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        Ok(Self { conn })
    }

    pub fn list_games(&self) -> Result<Vec<Game>> {
        let mut stmt = self.conn.prepare("
            SELECT appid, name, playtime_forever, last_played, installed
            FROM games
            ORDER BY installed DESC, name ASC
        ")?;

        let games = stmt.query_map([], |row| {
            Ok(Game {
                app_id:        row.get::<_, u32>(0)?,
                name:          row.get::<_, String>(1)?,
                playtime_mins: row.get::<_, u32>(2)?,
                last_played:   row.get::<_, u64>(3)?,
                installed:     row.get::<_, bool>(4)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();

        Ok(games)
    }

    pub fn mark_installed(&self, app_id: u32, installed: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE games SET installed=?1 WHERE appid=?2",
            params![installed as u8, app_id],
        )?;
        Ok(())
    }

    // ── Stats ────────────────────────────────

    pub fn total_games(&self) -> Result<u32> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM games", [], |r| r.get(0)
        )?)
    }

    pub fn total_installed(&self) -> Result<u32> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM games WHERE installed=1", [], |r| r.get(0)
        )?)
    }

    pub fn total_playtime_mins(&self) -> Result<u64> {
        Ok(self.conn.query_row(
            "SELECT COALESCE(SUM(playtime_forever),0) FROM games", [], |r| r.get(0)
        )?)
    }

    pub fn top_played(&self, limit: u32) -> Result<Vec<Game>> {
        let mut stmt = self.conn.prepare("
            SELECT appid, name, playtime_forever, last_played, installed
            FROM games
            ORDER BY playtime_forever DESC
            LIMIT ?1
        ")?;

        let games = stmt.query_map(params![limit], |row| {
            Ok(Game {
                app_id:        row.get::<_, u32>(0)?,
                name:          row.get::<_, String>(1)?,
                playtime_mins: row.get::<_, u32>(2)?,
                last_played:   row.get::<_, u64>(3)?,
                installed:     row.get::<_, bool>(4)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();

        Ok(games)
    }

    pub fn recently_played(&self, limit: u32) -> Result<Vec<Game>> {
        let mut stmt = self.conn.prepare("
            SELECT appid, name, playtime_forever, last_played, installed
            FROM games
            WHERE last_played > 0
            ORDER BY last_played DESC
            LIMIT ?1
        ")?;

        let games = stmt.query_map(params![limit], |row| {
            Ok(Game {
                app_id:        row.get::<_, u32>(0)?,
                name:          row.get::<_, String>(1)?,
                playtime_mins: row.get::<_, u32>(2)?,
                last_played:   row.get::<_, u64>(3)?,
                installed:     row.get::<_, bool>(4)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();

        Ok(games)
    }
}
