// main.rs — Entry point, event loop

mod app;
mod db;
mod image;
mod steam;
mod ui;

use app::{App, InstallState, Tab, WishlistEntry, WishlistSort};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use db::Database;
use image::ImageRenderer;
use ratatui::{backend::CrosstermBackend, Terminal};
use std::{
    io,
    path::PathBuf,
    sync::mpsc,
    thread,
    time::Duration,
};

fn main() -> anyhow::Result<()> {
    // Resolve project root FIRST, then load .env from it.
    // dotenv::dotenv() loads from CWD which is wrong when run from ~.
    let project_root = project_root();
    dotenv::from_path(project_root.join(".env")).ok();
    let config = steam::SteamConfig::from_env()?;
    let db_path      = project_root.join("steam_games.db");
    let db           = Database::open(&db_path)?;

    // ── Color palette ─────────────────────────
    // Prefer ~/.config/noguisteam/colors.json (XDG); fall back to project-root
    // colors.json. Missing files use the built-in palette.
    let palette_path = xdg_config_path("colors.json")
        .filter(|p| p.exists())
        .unwrap_or_else(|| project_root.join("colors.json"));
    ui::set_palette(ui::load_palette_from(&palette_path));

    // One shared HTTP client (connection pool, gzip, timeouts) used for
    // every Steam/ITAD/CDN request in the app.
    let http = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("noguisteam/0.1")
        .build()?;

    let mut app  = App::new(&db, config.steamlib.clone())?;
    let renderer = ImageRenderer::new(http.clone());

    // Restore terminal on panic so the user sees a usable shell + panic message
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::LeaveAlternateScreen,
            crossterm::event::DisableMouseCapture,
        );
        original_hook(panic_info);
    }));

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend  = CrosstermBackend::new(stdout);
    let mut term = Terminal::new(backend)?;

    let result = run(&mut term, &mut app, &db, &db_path, &config, &renderer, &http);

    disable_raw_mode()?;
    execute!(term.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    term.show_cursor()?;

    if let Err(e) = result {
        eprintln!("Error: {}", e);
    }
    Ok(())
}

// ─────────────────────────────────────────────
// Main loop
// ─────────────────────────────────────────────
fn run(
    term:     &mut Terminal<CrosstermBackend<io::Stdout>>,
    app:      &mut App,
    db:       &Database,
    db_path:  &PathBuf,
    config:   &steam::SteamConfig,
    renderer: &ImageRenderer,
    http:     &reqwest::blocking::Client,
) -> anyhow::Result<()> {

    let (install_tx, install_rx)   = mpsc::channel::<String>();
    let (wishlist_tx, wishlist_rx) = mpsc::channel::<Result<Vec<WishlistEntry>, String>>();
    let (sync_tx, sync_rx)         = mpsc::channel::<Result<usize, String>>();
    let mut last_rendered_appid: Option<u32> = None;

    // ── Hydrate wishlist from on-disk cache; auto-refresh if stale (>6h) ──
    const WISHLIST_TTL_SECS: u64 = 6 * 3600;
    if let Some((mut entries, age)) = db.load_wishlist_cache::<WishlistEntry>() {
        sort_wishlist_entries(&mut entries, &app.wishlist_sort);
        app.wishlist = entries;
        let mins = age / 60;
        let label = if mins < 60 { format!("{}m", mins) } else { format!("{}h", mins / 60) };
        app.set_status(format!("Wishlist cached ({} ago — press R to refresh).", label));

        if age > WISHLIST_TTL_SECS && config.itad_key.is_some() && !app.wishlist_loading {
            app.wishlist_loading = true;
            let cfg    = config.clone();
            let tx     = wishlist_tx.clone();
            let client = http.clone();
            let db_p   = db_path.clone();
            thread::spawn(move || {
                let result = (|| -> Result<Vec<WishlistEntry>, String> {
                    let db2 = Database::open(&db_p).map_err(|e| e.to_string())?;
                    let entries = steam::fetch_wishlist_sales(&client, &cfg, &db2)
                        .map_err(|e| e.to_string())?;
                    let _ = db2.save_wishlist_cache(&entries);
                    Ok(entries)
                })();
                let _ = tx.send(result);
            });
        }
    }

    // ── Optional auto-sync at startup (AUTO_SYNC=1 in .env) ──
    if config.auto_sync && !app.sync_loading {
        app.sync_loading = true;
        app.set_status("Auto-syncing library…");
        let db_p   = db_path.clone();
        let cfg    = config.clone();
        let tx     = sync_tx.clone();
        let client = http.clone();
        thread::spawn(move || {
            let result = steam::sync_library_to(&client, &cfg, &db_p)
                .map_err(|e| e.to_string());
            let _ = tx.send(result);
        });
    }

    loop {
        // ── Drain install subprocess output ──
        const INSTALL_OUTPUT_CAP: usize = 200;
        while let Ok(line) = install_rx.try_recv() {
            if let InstallState::Running { output, app_id } = &mut app.install_state {
                if line.starts_with("✅") {
                    let id = *app_id;
                    app.install_state = InstallState::Done { app_id: id, success: true };
                    let _ = app.reload_games(db);
                } else if line.to_lowercase().contains("error") || line.to_lowercase().contains("failed") {
                    let id = *app_id;
                    app.install_state = InstallState::Done { app_id: id, success: false };
                } else if line.contains("Waiting for confirmation") {
                    // Replace repeated "Waiting for confirmation" with a single clear message
                    if !output.iter().any(|l| l.contains("📱")) {
                        output.push("📱 Approve Steam Guard in your mobile app…".to_string());
                    }
                } else {
                    output.push(line);
                    // Keep memory bounded on huge installs (50GB games emit thousands of lines).
                    if output.len() > INSTALL_OUTPUT_CAP {
                        let drop = output.len() - INSTALL_OUTPUT_CAP;
                        output.drain(0..drop);
                    }
                }
            }
        }

        // ── Drain wishlist fetch result ──────
        if let Ok(result) = wishlist_rx.try_recv() {
            app.wishlist_loading = false;
            match result {
                Ok(mut entries) => {
                    sort_wishlist_entries(&mut entries, &app.wishlist_sort);
                    app.wishlist     = entries;
                    app.wishlist_sel = 0;
                    app.set_status(format!("{} sales found.", app.wishlist.len()));
                }
                Err(e) => app.set_status(format!("Error: {}", e)),
            }
        }

        // ── Drain library sync result ────────
        if let Ok(result) = sync_rx.try_recv() {
            app.sync_loading = false;
            match result {
                Ok(n)  => { let _ = app.reload_games(db); app.set_status(format!("Synced — {} games.", n)); }
                Err(e) => app.set_status(format!("Sync failed: {}", e)),
            }
        }

        // ── Drain image-fetch completions so the next paint picks up new cache hits
        let mut image_ready = false;
        while renderer.done_rx.try_recv().is_ok() { image_ready = true; }
        if image_ready { last_rendered_appid = None; }

        // ── Force full redraw if flagged ─────
        if app.needs_clear {
            app.needs_clear = false;
            term.clear()?;
        }

        // ── Draw TUI ─────────────────────────
        term.draw(|f| ui::draw(f, app))?;

        // ── Render image via chafa AFTER ratatui draw ──
        // chafa writes ANSI escape codes directly to stdout with absolute
        // cursor positioning. We re-render only when the selected game changes.
        if app.active_tab == Tab::Library && renderer.available {
            let current_appid = app.selected_game().map(|g| g.app_id);

            if current_appid != last_rendered_appid {
                renderer.clear();
                if let Some(game) = app.selected_game() {
                    let area = term.size().unwrap_or_default();
                    // Detail panel is fixed at 48 cols (right edge of terminal).
                    // Image is 46 cols wide (460px @ 10px/cell), starts 1 col in from border.
                    // col/row are 1-based for ANSI cursor positioning.
                    let col = area.width.saturating_sub(48) + 2; // panel start + border + 1-based
                    let row = 5u16; // tab bar (3) + border (1) + 1-based offset
                    let img_w = 46u16;
                    let img_h = 9u16;

                    renderer.render(game.app_id, col, row, img_w, img_h);
                }
                last_rendered_appid = current_appid;
            }
        } else if app.active_tab != Tab::Library && last_rendered_appid.is_some() {
            renderer.clear();
            last_rendered_appid = None;
        }

        // ── Poll events ──────────────────────
        if !event::poll(Duration::from_millis(100))? {
            continue;
        }

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Char('q') | KeyCode::Char('Q') if !app.search_mode => {
                    app.should_quit = true;
                }
                KeyCode::Tab => {
                    renderer.clear();
                    app.needs_clear = true;
                    app.next_tab();
                }
                KeyCode::BackTab => {
                    renderer.clear();
                    app.needs_clear = true;
                    app.prev_tab();
                }
                _ => {}
            }

            if app.should_quit { break; }

            match app.active_tab {
                Tab::Library  => handle_library(key, app, db_path, config, install_tx.clone(), sync_tx.clone(), http, &renderer, &mut last_rendered_appid),
                Tab::Wishlist => handle_wishlist(key, app, config, db_path, http, wishlist_tx.clone()),
                Tab::Stats    => {}
            }
        }
    }

    renderer.clear();
    Ok(())
}

// ─────────────────────────────────────────────
// Library key handling
// ─────────────────────────────────────────────
fn handle_library(
    key:              crossterm::event::KeyEvent,
    app:              &mut App,
    db_path:          &PathBuf,
    config:           &steam::SteamConfig,
    install_tx:       mpsc::Sender<String>,
    sync_tx:          mpsc::Sender<Result<usize, String>>,
    http:             &reqwest::blocking::Client,
    renderer:         &ImageRenderer,
    last_appid:       &mut Option<u32>,
) {
    if app.search_mode {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => { app.search_mode = false; }
            KeyCode::Backspace => { app.search.pop(); app.apply_filter(); }
            KeyCode::Char(c)   => { app.search.push(c); app.apply_filter(); }
            _ => {}
        }
        return;
    }

    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            renderer.clear();
            *last_appid = None;
            app.move_up();
        }
        KeyCode::Down | KeyCode::Char('j') => {
            renderer.clear();
            *last_appid = None;
            app.move_down();
        }
        KeyCode::Char('/') => {
            app.search_mode = true;
        }
        KeyCode::Char('i') | KeyCode::Char('I') => {
            if let Some(game) = app.selected_game().cloned() {
                if game.installed {
                    app.set_status(format!("{} is already installed.", game.name));
                    return;
                }
                let app_id     = game.app_id;
                let tx         = install_tx.clone();
                let db_p       = db_path.clone();
                let cfg_clone  = config.clone();

                let mut startup = vec![format!("Starting install for {}…", game.name)];
                if let Some(free) = steam::free_bytes_for(&config.steamlib) {
                    let median = steam::installed_median_size(&config.steamlib);
                    startup.push(format!(
                        "💾 {} free on Steam library partition (median installed game: {}).",
                        steam::human_bytes(free),
                        steam::human_bytes(median),
                    ));
                    if free < median.saturating_mul(2) {
                        startup.push("⚠️  Low disk space — install may fail.".to_string());
                    }
                }

                app.install_state = InstallState::Running {
                    app_id,
                    output: startup,
                };

                thread::spawn(move || {
                    let db2 = match Database::open(&db_p) {
                        Ok(d)  => d,
                        Err(e) => { let _ = tx.send(format!("error: failed to open DB: {}", e)); return; }
                    };
                    if let Err(e) = steam::install_game(app_id, &cfg_clone, &db2, tx.clone()) {
                        let _ = tx.send(format!("error: {}", e));
                    }
                });
            }
        }
        KeyCode::Char('u') | KeyCode::Char('U') => {
            if let Some(game) = app.selected_game().cloned() {
                if !game.installed {
                    app.set_status(format!("{} is not installed.", game.name));
                    return;
                }
                let app_id    = game.app_id;
                let db_p      = db_path.clone();
                let name      = game.name.clone();
                let tx        = install_tx.clone();
                let cfg_clone = config.clone();

                app.install_state = InstallState::Running {
                    app_id,
                    output: vec![format!("Uninstalling {} via steamcmd…", name)],
                };

                thread::spawn(move || {
                    let db2 = match Database::open(&db_p) {
                        Ok(d)  => d,
                        Err(e) => { let _ = tx.send(format!("error: failed to open DB: {}", e)); return; }
                    };
                    match steam::uninstall_game(app_id, &cfg_clone, &db2) {
                        Ok(_)  => { let _ = tx.send(format!("✅ {} uninstalled.", name)); }
                        Err(e) => { let _ = tx.send(format!("error: {}", e)); }
                    }
                });
            }
        }
        KeyCode::Char('p') | KeyCode::Char('P') => {
            if let Some(game) = app.selected_game() {
                let _ = std::process::Command::new("steam")
                    .args(["-silent", &format!("steam://run/{}", game.app_id)])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
                app.set_status(format!("Launching {}…", game.name));
                app.needs_clear = true;
            }
        }
        KeyCode::Char('l') | KeyCode::Char('L') => {
            if app.sync_loading {
                app.set_status("Already syncing…".to_string());
                return;
            }
            app.sync_loading = true;
            app.set_status("Syncing library… (you can keep using the UI)".to_string());

            let db_p      = db_path.clone();
            let cfg_clone = config.clone();
            let tx        = sync_tx.clone();
            let client    = http.clone();
            thread::spawn(move || {
                let result = steam::sync_library_to(&client, &cfg_clone, &db_p)
                    .map_err(|e| e.to_string());
                let _ = tx.send(result);
            });
        }
        KeyCode::Esc => {
            if !app.search.is_empty() { app.search.clear(); app.apply_filter(); }
            app.clear_status();
        }
        // Dismiss install popup on any key
        _ if matches!(&app.install_state, InstallState::Done { .. }) => {
            app.install_state = InstallState::Idle;
            renderer.clear();
            *last_appid = None;
            app.needs_clear = true;
        }
        _ => {}
    }
}

// ─────────────────────────────────────────────
// Wishlist key handling
// ─────────────────────────────────────────────
fn handle_wishlist(
    key: crossterm::event::KeyEvent,
    app: &mut App,
    config: &steam::SteamConfig,
    db_path: &PathBuf,
    http: &reqwest::blocking::Client,
    wishlist_tx: mpsc::Sender<Result<Vec<WishlistEntry>, String>>,
) {
    match key.code {
        KeyCode::Up   | KeyCode::Char('k') => {
            if app.wishlist_sel > 0 {
                app.wishlist_sel -= 1;
                show_wishlist_url(app);
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.wishlist_sel + 1 < app.wishlist.len() {
                app.wishlist_sel += 1;
                show_wishlist_url(app);
            }
        }
        KeyCode::Char('s') | KeyCode::Char('S') => {
            app.wishlist_sort = match app.wishlist_sort {
                WishlistSort::Deal     => WishlistSort::Discount,
                WishlistSort::Discount => WishlistSort::Price,
                WishlistSort::Price    => WishlistSort::Deal,
            };
            app.wishlist_sel = 0;
            sort_wishlist(app);
        }
        KeyCode::Char('o') | KeyCode::Char('O') => {
            if let Some(entry) = app.wishlist.get(app.wishlist_sel) {
                let url = entry.url.clone();
                let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
                match std::process::Command::new(opener)
                    .arg(&url)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                {
                    Ok(_)  => {
                        app.set_status(format!("Opening: {}", url));
                        app.needs_clear = true; // xdg-open can briefly corrupt the buffer
                    }
                    Err(e) => app.set_status(format!("Failed to open URL: {}", e)),
                }
            }
        }
        KeyCode::Char('r') | KeyCode::Char('R') => {
            if app.wishlist_loading {
                app.set_status("Already fetching wishlist…".to_string());
                return;
            }
            app.wishlist_loading = true;
            app.wishlist.clear();
            app.wishlist_sel = 0;
            app.set_status("Fetching wishlist… (you can switch tabs)".to_string());

            let cfg      = config.clone();
            let tx       = wishlist_tx.clone();
            let client   = http.clone();
            let db_p     = db_path.clone();
            thread::spawn(move || {
                let result = (|| -> Result<Vec<WishlistEntry>, String> {
                    let db2 = Database::open(&db_p).map_err(|e| e.to_string())?;
                    let entries = steam::fetch_wishlist_sales(&client, &cfg, &db2)
                        .map_err(|e| e.to_string())?;
                    // Refresh on-disk cache; ignore failure since the data is still good.
                    let _ = db2.save_wishlist_cache(&entries);
                    Ok(entries)
                })();
                let _ = tx.send(result);
            });
        }
        _ => {}
    }
}

fn show_wishlist_url(app: &mut App) {
    if let Some(entry) = app.wishlist.get(app.wishlist_sel) {
        app.set_status(format!("O: open  |  {}", entry.url));
    }
}

fn sort_wishlist(app: &mut App) {
    sort_wishlist_entries(&mut app.wishlist, &app.wishlist_sort);
    app.wishlist_sel = 0;
}

fn sort_wishlist_entries(entries: &mut Vec<app::WishlistEntry>, sort: &WishlistSort) {
    match sort {
        WishlistSort::Deal     => entries.sort_by(|a, b| deal_order(&a.deal_tag).cmp(&deal_order(&b.deal_tag)).then(a.current_price.total_cmp(&b.current_price))),
        WishlistSort::Discount => entries.sort_by(|a, b| b.discount_percent.cmp(&a.discount_percent)),
        WishlistSort::Price    => entries.sort_by(|a, b| a.current_price.total_cmp(&b.current_price)),
    }
}

fn deal_order(tag: &str) -> u8 {
    if tag.contains("All-Time") { 0 } else if tag.contains("Cross") { 1 } else if tag.contains("Match") { 2 } else { 3 }
}

// ─────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────
fn xdg_config_path(file: &str) -> Option<PathBuf> {
    let base = std::env::var("XDG_CONFIG_HOME").ok()
        .map(PathBuf::from)
        .or_else(|| std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("noguisteam").join(file))
}

fn project_root() -> PathBuf {
    if let Ok(home) = std::env::var("NOGUISTEAM_HOME") {
        let p = PathBuf::from(home);
        if p.join(".env").exists() || p.join("steam_games.db").exists() {
            return p;
        }
    }
    let exe = std::env::current_exe().unwrap_or_default();
    let mut dir = exe.parent().unwrap_or(std::path::Path::new(".")).to_path_buf();
    for _ in 0..6 {
        if dir.join("steam_games.db").exists() || dir.join(".env").exists() {
            return dir;
        }
        if let Some(p) = dir.parent() { dir = p.to_path_buf(); } else { break; }
    }
    std::env::current_dir().unwrap_or_default()
}
