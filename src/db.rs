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
        Self::migrate(&conn)?;
        Ok(Self { conn })
    }

    /// Runs ordered, idempotent schema migrations driven by `PRAGMA user_version`.
    /// Add a new step to advance the version; never edit an existing step.
    fn migrate(conn: &Connection) -> Result<()> {
        let current: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;

        let steps: &[(&str, &str)] = &[
            // v1 — initial games table
            ("v1", "
                CREATE TABLE IF NOT EXISTS games (
                    appid            INTEGER PRIMARY KEY,
                    name             TEXT,
                    playtime_forever INTEGER,
                    last_played      INTEGER,
                    installed        BOOLEAN
                );
            "),
            // v2 — wishlist cache (single-row JSON blob, keyed by 1)
            ("v2", "
                CREATE TABLE IF NOT EXISTS wishlist_cache (
                    id         INTEGER PRIMARY KEY,
                    payload    TEXT NOT NULL,
                    fetched_at INTEGER NOT NULL
                );
            "),
        ];

        for (i, (_label, sql)) in steps.iter().enumerate() {
            let v = (i + 1) as u32;
            if current < v {
                conn.execute_batch(sql)?;
                conn.execute_batch(&format!("PRAGMA user_version = {};", v))?;
            }
        }
        Ok(())
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Look up a name by appid from the cached library, bypassing the slow
    /// Steam `appdetails` endpoint when possible.
    pub fn name_for(&self, app_id: u32) -> Option<String> {
        self.conn.query_row(
            "SELECT name FROM games WHERE appid = ?1",
            params![app_id],
            |r| r.get::<_, String>(0),
        ).ok()
    }

    /// Persist the latest wishlist entries with a fetch timestamp.
    pub fn save_wishlist_cache<T: serde::Serialize>(&self, entries: &[T]) -> Result<()> {
        let json = serde_json::to_string(entries)?;
        let now  = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0) as i64;
        self.conn.execute(
            "INSERT OR REPLACE INTO wishlist_cache (id, payload, fetched_at) VALUES (1, ?1, ?2)",
            params![json, now],
        )?;
        Ok(())
    }

    /// Returns (entries, age_seconds) if a cache row exists.
    pub fn load_wishlist_cache<T: for<'de> serde::Deserialize<'de>>(&self) -> Option<(Vec<T>, u64)> {
        let (json, fetched_at): (String, i64) = self.conn.query_row(
            "SELECT payload, fetched_at FROM wishlist_cache WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        ).ok()?;
        let entries: Vec<T> = serde_json::from_str(&json).ok()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let age = now.saturating_sub(fetched_at as u64);
        Some((entries, age))
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
