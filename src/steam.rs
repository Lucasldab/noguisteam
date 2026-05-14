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
    /// Optional: if unset, steamcmd uses cached credentials from a prior
    /// interactive `+login user` (preferred — keeps the password off disk).
    pub password:  Option<String>,
    pub itad_key:  Option<String>,
    pub country:   String,
    pub steamcmd:  PathBuf,
    pub steamlib:  PathBuf,
    pub auto_sync: bool,
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
            password:    std::env::var("STEAM_PASSWORD").ok().filter(|s| !s.is_empty()),
            itad_key:    std::env::var("ITAD_KEY").ok(),
            country:     std::env::var("COUNTRY").unwrap_or_else(|_| "US".into()),
            steamcmd:    PathBuf::from(
                            std::env::var("STEAMCMD")
                                .unwrap_or_else(|_| "/usr/bin/steamcmd".into())
                        ),
            steamlib:    PathBuf::from(format!("{}/.steam/steam/steamapps", home)),
            auto_sync:   matches!(
                std::env::var("AUTO_SYNC").unwrap_or_default().to_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            ),
        })
    }
}

/// Build the steamcmd `+login` arguments. Omits the password when not set so
/// steamcmd uses its cached session instead.
fn login_args(config: &SteamConfig) -> Vec<String> {
    let mut v = vec!["+login".to_string(), config.username.clone()];
    if let Some(pw) = &config.password {
        v.push(pw.clone());
    }
    v
}

/// Bytes free on the filesystem holding `path`. Uses `df -B1` to avoid pulling
/// in a libc/nix dependency. Returns None if df is unavailable or output is
/// unparseable.
pub fn free_bytes_for(path: &Path) -> Option<u64> {
    let out = Command::new("df")
        .args(["-B1", "--output=avail"])
        .arg(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() { return None; }
    let text = String::from_utf8_lossy(&out.stdout);
    // Format:
    //   Avail
    //   123456789
    text.lines().nth(1)?.trim().parse::<u64>().ok()
}

/// Best-effort install size estimate. Steam's store API returns size as a
/// localized free-text string ("Storage: 50 GB") that we can't parse robustly,
/// so we use a coarse heuristic: median SizeOnDisk of currently-installed
/// games (in bytes), or a 10 GB fallback when nothing is installed.
pub fn installed_median_size(steamlib: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(steamlib) else { return 10 * 1024u64.pow(3); };
    let mut sizes: Vec<u64> = Vec::new();
    for entry in rd.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("appmanifest_") || !name.ends_with(".acf") { continue; }
        let path = entry.path();
        let Ok(text) = std::fs::read_to_string(&path) else { continue; };
        if let Some(size) = parse_acf_size(&text) {
            sizes.push(size);
        }
    }
    if sizes.is_empty() { return 10 * 1024u64.pow(3); }
    sizes.sort_unstable();
    sizes[sizes.len() / 2]
}

fn parse_acf_size(text: &str) -> Option<u64> {
    // VDF lines look like: "\t\"SizeOnDisk\"\t\t\"1234567\""
    for line in text.lines() {
        let l = line.trim();
        if !l.starts_with("\"SizeOnDisk\"") { continue; }
        // Split out the quoted value at end of line.
        let last_quote_end = l.rfind('"')?;
        let pre = &l[..last_quote_end];
        let last_quote_start = pre.rfind('"')?;
        return l[last_quote_start + 1 .. last_quote_end].parse::<u64>().ok();
    }
    None
}

/// On-disk byte size of an installed game, from its appmanifest.
pub fn game_size_on_disk(steamlib: &Path, app_id: u32) -> Option<u64> {
    let path = steamlib.join(format!("appmanifest_{}.acf", app_id));
    let text = std::fs::read_to_string(&path).ok()?;
    parse_acf_size(&text)
}

pub fn human_bytes(n: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 { v /= 1024.0; i += 1; }
    if i == 0 { format!("{} {}", n, UNITS[i]) } else { format!("{:.1} {}", v, UNITS[i]) }
}

// ─────────────────────────────────────────────
// Install / Uninstall
// ─────────────────────────────────────────────

/// Strip ANSI escape sequences and progress-bar carriage returns from a line
/// of steamcmd output. Steamcmd uses `\r` to redraw progress in place, so a
/// single `\n`-terminated line can carry many overwritten frames plus CSI
/// codes; rendering that raw produces visible garbage in the popup.
fn sanitize_line(input: &str) -> String {
    let frame = input.rsplit('\r').next().unwrap_or(input);

    let mut out = String::with_capacity(frame.len());
    let mut chars = frame.chars();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => {
                // Drop ANSI CSI sequence: ESC [ ... <final byte 0x40..=0x7E>.
                if chars.next() == Some('[') {
                    for b in chars.by_ref() {
                        if matches!(b, '\x40'..='\x7e') { break; }
                    }
                }
            }
            '\t' => out.push(' '),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out.trim().to_string()
}

/// Returns true if a `steam` process is currently running on this machine.
fn is_steam_running() -> bool {
    Command::new("pgrep")
        .args(["-x", "steam"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// If Steam is running, ask it to shut down gracefully and relaunch silently.
/// This makes the client re-scan `steamapps/` so a freshly installed game
/// shows up in the library without the user restarting Steam manually.
fn restart_steam_if_running(tx: &Sender<String>) {
    if !is_steam_running() {
        return;
    }
    let _ = tx.send("🔄 Restarting Steam to register new game…".to_string());

    let _ = Command::new("steam")
        .arg("-shutdown")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .status();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while is_steam_running() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(500));
    }

    let _ = Command::new("steam")
        .arg("-silent")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

/// Runs steamcmd for install, sending each line of output to `tx` as it arrives.
/// Password is passed to avoid interactive login prompts. Steam Guard may still
/// require confirmation via the mobile app (SteamCMD waits automatically).
pub fn install_game(
    app_id: u32,
    config: &SteamConfig,
    db: &Database,
    tx: Sender<String>,
) -> Result<()> {
    let mut args = login_args(config);
    args.extend(["+app_update".into(), app_id.to_string(), "validate".into(), "+quit".into()]);

    let mut child = Command::new(&config.steamcmd)
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn()
        .context("Failed to launch steamcmd")?;

    if let Some(stdout) = child.stdout.take() {
        use std::io::{BufRead, BufReader};
        for line in BufReader::new(stdout).lines().flatten() {
            let cleaned = sanitize_line(&line);
            if !cleaned.is_empty() {
                let _ = tx.send(cleaned);
            }
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
        restart_steam_if_running(&tx);
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
    let mut args = login_args(config);
    args.extend(["+app_uninstall".into(), app_id.to_string(), "+quit".into()]);

    let status = std::process::Command::new(&config.steamcmd)
        .args(&args)
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

/// Preferred version — takes the DB path directly. Caller supplies the HTTP
/// client so a single connection pool is shared across the app.
pub fn sync_library_to(client: &reqwest::blocking::Client, config: &SteamConfig, db_path: &Path) -> Result<usize> {
    let resp: OwnedGamesResponse = client
        .get("https://api.steampowered.com/IPlayerService/GetOwnedGames/v1/")
        .query(&[
            ("key",                       config.api_key.as_str()),
            ("steamid",                   config.steam_id.as_str()),
            ("include_appinfo",           "true"),
            ("include_played_free_games", "true"),
            ("include_free_sub",          "true"),
            ("skip_unvetted_apps",        "false"),
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

    // Schema is owned by Database::migrate; open through it so migrations run.
    let db = Database::open(db_path)?;
    let conn = db.conn();

    let tx = conn.unchecked_transaction()?;
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
    let stale_ids: Vec<u32> = {
        let mut stmt = conn.prepare("SELECT appid FROM games WHERE installed=1")?;
        let ids: Vec<u32> = stmt.query_map([], |r| r.get(0))?
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

    tx.commit()?;
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

pub fn fetch_wishlist_sales(
    client: &reqwest::blocking::Client,
    config: &SteamConfig,
    db: &Database,
) -> Result<Vec<WishlistEntry>> {
    let itad_key = config.itad_key.as_ref()
        .ok_or_else(|| anyhow!("ITAD_KEY not set in .env"))?;

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

    // 2. Resolve names — prefer the synced library DB to avoid the per-appid
    //    `appdetails` storm (rate-limits at ~200/day). Wishlist items the user
    //    doesn't own won't be in the DB; fetch those individually as a fallback.
    let mut names: std::collections::HashMap<u32, String> = std::collections::HashMap::new();
    for &appid in &app_ids {
        if let Some(name) = db.name_for(appid) {
            names.insert(appid, name);
        }
    }
    let missing: Vec<u32> = app_ids.iter().copied().filter(|id| !names.contains_key(id)).collect();
    for appid in &missing {
        let url = format!(
            "https://store.steampowered.com/api/appdetails?appids={}&filters=basic",
            appid
        );
        if let Ok(resp) = client.get(&url).send() {
            if let Ok(map) = resp.json::<std::collections::HashMap<String, AppDetailsWrapper>>() {
                if let Some(wrapper) = map.get(&appid.to_string()) {
                    if wrapper.success {
                        if let Some(data) = &wrapper.data {
                            names.insert(*appid, data.name.clone());
                        }
                    }
                }
            }
        }
        names.entry(*appid).or_insert_with(|| format!("AppID {}", appid));
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
