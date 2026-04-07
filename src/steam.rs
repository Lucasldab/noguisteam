// steam.rs — steamcmd subprocess, library sync, wishlist API

use crate::app::WishlistEntry;
use crate::db::Database;
use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;

// ─────────────────────────────────────────────
// Config (from .env)
// ─────────────────────────────────────────────
#[derive(Clone)]
pub struct SteamConfig {
    pub api_key:   String,
    pub steam_id:  String,
    pub username:  String,
    pub password:  String,
    pub itad_key:  Option<String>,
    pub country:   String,
    pub steamcmd:  PathBuf,
    pub steamlib:  PathBuf,
}

impl SteamConfig {
    pub fn from_env() -> Result<Self> {
        // .env is already loaded by main() via dotenv::from_path — don't reload from CWD
        let home = std::env::var("HOME").unwrap_or_default();

        Ok(Self {
            api_key:     std::env::var("STEAM_API_KEY")
                            .context("STEAM_API_KEY not set in .env")?,
            steam_id:    std::env::var("STEAM_ID")
                            .context("STEAM_ID not set in .env")?,
            username:    std::env::var("STEAM_USERNAME")
                            .context("STEAM_USERNAME not set in .env")?,
            password:    std::env::var("STEAM_PASSWORD")
                            .context("STEAM_PASSWORD not set in .env")?,
            itad_key:    std::env::var("ITAD_KEY").ok(),
            country:     std::env::var("COUNTRY").unwrap_or_else(|_| "US".into()),
            steamcmd:    PathBuf::from(
                            std::env::var("STEAMCMD")
                                .unwrap_or_else(|_| "/usr/bin/steamcmd".into())
                        ),
            steamlib:    PathBuf::from(format!("{}/.steam/steam/steamapps", home)),
        })
    }
}

// ─────────────────────────────────────────────
// Install / Uninstall
// ─────────────────────────────────────────────

/// Runs steamcmd for install, sending each line of output to `tx` as it arrives.
/// Password is passed to avoid interactive login prompts. Steam Guard may still
/// require confirmation via the mobile app (SteamCMD waits automatically).
pub fn install_game(
    app_id: u32,
    config: &SteamConfig,
    db: &Database,
    tx: Sender<String>,
) -> Result<()> {
    let mut child = Command::new(&config.steamcmd)
        .args([
            "+login",      &config.username, &config.password,
            "+app_update", &app_id.to_string(),
            "validate",
            "+quit",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn()
        .context("Failed to launch steamcmd")?;

    if let Some(stdout) = child.stdout.take() {
        use std::io::{BufRead, BufReader};
        for line in BufReader::new(stdout).lines().flatten() {
            let _ = tx.send(line);
        }
    }

    let status = child.wait()?;
    if !status.success() {
        return Err(anyhow!("steamcmd exited with status {}", status));
    }

    // Re-check manifest after install
    let manifest = config.steamlib.join(format!("appmanifest_{}.acf", app_id));
    if manifest.exists() {
        db.mark_installed(app_id, true)?;
        let _ = tx.send(format!("✅ {} installed successfully.", app_id));
    } else {
        return Err(anyhow!("Manifest not found after install — Steam may not have recognized the game."));
    }

    Ok(())
}

pub fn uninstall_game(app_id: u32, config: &SteamConfig, db: &Database) -> Result<()> {
    // Use steamcmd app_uninstall so the running Steam client is notified.
    // Manually removing files works on disk but Steam keeps the game cached
    // in memory and still allows launching until it restarts.
    let status = std::process::Command::new(&config.steamcmd)
        .args([
            "+login",          &config.username, &config.password,
            "+app_uninstall",  &app_id.to_string(),
            "+quit",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .status()
        .context("Failed to launch steamcmd for uninstall")?;

    if !status.success() {
        return Err(anyhow!("steamcmd app_uninstall failed with status {}", status));
    }

    // steamcmd removes the files and manifest itself.
    // Belt-and-suspenders: remove manifest if it somehow still exists.
    let manifest = config.steamlib.join(format!("appmanifest_{}.acf", app_id));
    if manifest.exists() {
        std::fs::remove_file(&manifest)?;
    }

    db.mark_installed(app_id, false)?;
    Ok(())
}

// ─────────────────────────────────────────────
// Library sync (replaces sync.py)
// ─────────────────────────────────────────────
#[derive(Deserialize)]
struct OwnedGamesResponse {
    response: OwnedGamesInner,
}
#[derive(Deserialize)]
struct OwnedGamesInner {
    #[serde(default)]
    games: Vec<SteamGame>,
}
#[derive(Deserialize)]
struct SteamGame {
    appid:            u32,
    name:             String,
    playtime_forever: u32,
    #[serde(default)]
    rtime_last_played: u64,
}

/// Preferred version — takes the DB path directly.
pub fn sync_library_to(config: &SteamConfig, db_path: &Path) -> Result<usize> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("Failed to build HTTP client")?;

    let resp: OwnedGamesResponse = client
        .get("https://api.steampowered.com/IPlayerService/GetOwnedGames/v1/")
        .query(&[
            ("key",                       config.api_key.as_str()),
            ("steamid",                   config.steam_id.as_str()),
            ("include_appinfo",           "true"),
            ("include_played_free_games", "true"),
        ])
        .send()
        .context("Steam API request failed")?
        .json()
        .context("Failed to parse Steam API response")?;

    let games = resp.response.games;

    // Guard: Steam returns empty response for private profiles or wrong Steam ID.
    // If API returns 0 games but DB already has games, abort to prevent data loss.
    if games.is_empty() {
        let conn = rusqlite::Connection::open(db_path)?;
        let existing: i64 = conn
            .query_row("SELECT COUNT(*) FROM games", [], |r| r.get(0))
            .unwrap_or(0);
        if existing > 0 {
            return Err(anyhow!(
                "Steam API returned 0 games — profile may be private or Steam ID is wrong. \
                 Sync aborted to protect your library ({} games).",
                existing
            ));
        }
    }

    let count = games.len();

    let conn = rusqlite::Connection::open(db_path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL;")?;
    conn.execute_batch("
        CREATE TABLE IF NOT EXISTS games (
            appid            INTEGER PRIMARY KEY,
            name             TEXT,
            playtime_forever INTEGER,
            last_played      INTEGER,
            installed        BOOLEAN
        );
    ")?;

    for g in &games {
        let manifest = config.steamlib.join(format!("appmanifest_{}.acf", g.appid));
        let installed = manifest.exists() as u8;

        conn.execute("
            INSERT OR REPLACE INTO games (appid, name, playtime_forever, last_played, installed)
            VALUES (?1, ?2, ?3, ?4, ?5)
        ", rusqlite::params![
            g.appid, g.name, g.playtime_forever, g.rtime_last_played, installed
        ])?;
    }

    // Clear stale installed flags
    // Note: stmt must be dropped before conn.close(), so we collect first
    let stale_ids: Vec<u32> = {
        let mut stmt = conn.prepare("SELECT appid FROM games WHERE installed=1")?;
        let ids = stmt.query_map([], |r| r.get(0))?
            .filter_map(|r| r.ok())
            .collect();
        ids
    };

    for appid in stale_ids {
        let manifest = config.steamlib.join(format!("appmanifest_{}.acf", appid));
        if !manifest.exists() {
            conn.execute(
                "UPDATE games SET installed=0 WHERE appid=?1",
                rusqlite::params![appid],
            )?;
        }
    }

    conn.close().map_err(|(_, e)| e)?;
    Ok(count)
}

// ─────────────────────────────────────────────
// Wishlist + ITAD (replaces wishlist.py)
// ─────────────────────────────────────────────
const ITAD_STEAM_SHOP_ID: u32 = 61;

#[derive(Deserialize)]
struct WishlistResponse {
    response: WishlistInner,
}
#[derive(Deserialize)]
struct WishlistInner {
    #[serde(default)]
    items: Vec<WishlistItem>,
}
#[derive(Deserialize)]
struct WishlistItem {
    appid: u32,
}
#[derive(Deserialize)]
struct AppDetailsWrapper {
    success: bool,
    data:    Option<AppDetailsData>,
}
#[derive(Deserialize)]
struct AppDetailsData {
    name: String,
}
#[derive(Deserialize)]
struct ItadPriceItem {
    id:     String,
    #[serde(default)]
    deals:  Vec<ItadDeal>,
    #[serde(rename = "historyLow")]
    history_low: Option<ItadHistoryLow>,
}
#[derive(Deserialize)]
struct ItadDeal {
    price:    ItadAmount,
    regular:  ItadAmount,
    cut:      u32,
    url:      Option<String>,
    #[serde(rename = "storeLow")]
    store_low: Option<ItadAmount>,
}
#[derive(Deserialize)]
struct ItadAmount {
    amount: f64,
}
#[derive(Deserialize)]
struct ItadHistoryLow {
    all: Option<ItadAmount>,
    y1:  Option<ItadAmount>,
}

pub fn fetch_wishlist_sales(config: &SteamConfig) -> Result<Vec<WishlistEntry>> {
    let itad_key = config.itad_key.as_ref()
        .ok_or_else(|| anyhow!("ITAD_KEY not set in .env"))?;

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("Failed to build HTTP client")?;

    // 1. Fetch wishlist
    let wl: WishlistResponse = client
        .get("https://api.steampowered.com/IWishlistService/GetWishlist/v1")
        .query(&[("key", &config.api_key), ("steamid", &config.steam_id)])
        .send()?
        .json()?;

    let app_ids: Vec<u32> = wl.response.items.iter().map(|i| i.appid).collect();
    if app_ids.is_empty() {
        return Ok(vec![]);
    }

    // 2. Resolve names
    let mut names: std::collections::HashMap<u32, String> = std::collections::HashMap::new();
    for &appid in &app_ids {
        let url = format!(
            "https://store.steampowered.com/api/appdetails?appids={}&filters=basic",
            appid
        );
        if let Ok(resp) = client.get(&url).send() {
            if let Ok(map) = resp.json::<std::collections::HashMap<String, AppDetailsWrapper>>() {
                if let Some(wrapper) = map.get(&appid.to_string()) {
                    if wrapper.success {
                        if let Some(data) = &wrapper.data {
                            names.insert(appid, data.name.clone());
                        }
                    }
                }
            }
        }
        names.entry(appid).or_insert_with(|| format!("AppID {}", appid));
    }

    // 3. ITAD ID lookup
    let shop_ids: Vec<String> = app_ids.iter().map(|id| format!("app/{}", id)).collect();
    let itad_lookup_resp = client
        .post(format!("https://api.isthereanydeal.com/lookup/id/shop/{}/v1", ITAD_STEAM_SHOP_ID))
        .query(&[("key", itad_key.as_str())])
        .json(&shop_ids)
        .send()
        .context("ITAD lookup request failed")?;
    let itad_lookup_text = itad_lookup_resp.text().context("ITAD lookup: failed to read body")?;
    let itad_map: std::collections::HashMap<String, Option<String>> =
        serde_json::from_str(&itad_lookup_text)
            .with_context(|| format!("ITAD lookup: failed to decode JSON: {}", &itad_lookup_text[..itad_lookup_text.len().min(500)]))?;

    // steam appid → itad uuid
    let steam_to_itad: std::collections::HashMap<u32, String> = itad_map
        .into_iter()
        .filter_map(|(k, v)| {
            let uuid = v?;
            let appid: u32 = k.trim_start_matches("app/").parse().ok()?;
            Some((appid, uuid))
        })
        .collect();

    let itad_to_steam: std::collections::HashMap<String, u32> = steam_to_itad
        .iter()
        .map(|(&ref k, v)| (v.clone(), *k))
        .collect();

    let itad_ids: Vec<String> = steam_to_itad.values().cloned().collect();

    // 4. Fetch prices — ITAD limit is 200 IDs per request, so chunk
    const ITAD_CHUNK: usize = 200;
    let mut price_items: Vec<ItadPriceItem> = Vec::new();

    for chunk in itad_ids.chunks(ITAD_CHUNK) {
        let prices_resp = client
            .post("https://api.isthereanydeal.com/games/prices/v3")
            .query(&[
                ("key",     itad_key.as_str()),
                ("country", config.country.as_str()),
                ("shops",   &ITAD_STEAM_SHOP_ID.to_string()),
                ("deals",   "1"),
            ])
            .json(&chunk)
            .send()
            .context("ITAD prices request failed")?;
        let prices_text = prices_resp.text().context("ITAD prices: failed to read body")?;
        let mut chunk_items: Vec<ItadPriceItem> =
            serde_json::from_str(&prices_text)
                .with_context(|| format!("ITAD prices: failed to decode JSON: {}", &prices_text[..prices_text.len().min(500)]))?;
        price_items.append(&mut chunk_items);
    }

    // 5. Assemble results
    let mut results = Vec::new();
    for item in price_items {
        if item.deals.is_empty() {
            continue;
        }

        let best = item.deals.iter()
            .min_by(|a, b| a.price.amount.total_cmp(&b.price.amount))
            .ok_or_else(|| anyhow!("No deals found for game despite non-empty check"))?;

        let historical_low = item.history_low.as_ref()
            .and_then(|h| h.all.as_ref())
            .map(|a| a.amount);
        let one_year_low = item.history_low.as_ref()
            .and_then(|h| h.y1.as_ref())
            .map(|a| a.amount);
        let store_low = best.store_low.as_ref().map(|a| a.amount);

        let current = best.price.amount;
        let deal_tag = classify_deal(current, store_low, historical_low, one_year_low);

        let steam_appid = itad_to_steam.get(&item.id).copied().unwrap_or(0);
        let name = names.get(&steam_appid)
            .cloned()
            .unwrap_or_else(|| format!("ITAD:{}", item.id));

        // Steam store URL: prefer ITAD deal URL, fall back to constructed store link
        let url = best.url.clone()
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| format!("https://store.steampowered.com/app/{}", steam_appid));

        results.push(WishlistEntry {
            name,
            current_price:    current,
            regular_price:    best.regular.amount,
            discount_percent: best.cut,
            deal_tag,
            store_low,
            historical_low,
            url,
        });
    }

    Ok(results)
}

fn classify_deal(
    current:      f64,
    store_low:    Option<f64>,
    hist_low:     Option<f64>,
    one_year_low: Option<f64>,
) -> String {
    if store_low.map_or(false, |l| current <= l) {
        return "🔥 Steam All-Time Low".into();
    }
    if hist_low.map_or(false, |l| current <= l) {
        return "🏆 Cross-Store Low".into();
    }
    if one_year_low.map_or(false, |l| current <= l) {
        return "🔁 Matching Lowest".into();
    }
    "🏷  On Sale".into()
}
