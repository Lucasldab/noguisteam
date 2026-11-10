// main.rs — Entry point, event loop

mod app;
mod cover;
mod db;
mod setup;
mod steam;
mod steamcmd;
mod ui;

use app::{App, InstallMsg, InstallState, Tab, WishlistEntry, WishlistSort};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use db::Database;
use cover::ImageRenderer;
use ratatui::{backend::CrosstermBackend, Terminal};
use std::{
    io,
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::Duration,
};

const USAGE: &str = "\
Usage: noguisteam [login]

  (no command)  open the TUI
  login         log steamcmd in once (asks for password + Steam Guard);
                installs and uninstalls then use the cached session

Settings: ~/.config/noguisteam/env (created on first run)
";

fn main() -> anyhow::Result<()> {
    let cmd = std::env::args().nth(1);
    if matches!(cmd.as_deref(), Some("-h" | "--help" | "help")) {
        print!("{USAGE}");
        return Ok(());
    }

    let paths = setup::Paths::resolve();
    if setup::write_env_template(&paths.env_file)? {
        println!("Created {} — put your Steam Web API key in it", paths.env_file.display());
        println!("(https://steamcommunity.com/dev/apikey), then run noguisteam again.");
        return Ok(());
    }
    setup::make_private(&paths.env_file);
    // Real environment variables win over the file.
    dotenv::from_path(&paths.env_file).ok();
    if std::env::var_os("STEAM_PASSWORD").is_some() {
        eprintln!("note: STEAM_PASSWORD is ignored — run `noguisteam login` once instead, and remove it from {}",
                  paths.env_file.display());
    }

    match cmd.as_deref() {
        None => {}
        Some("login") => {
            let (steamcmd, username) = steam::login_target(&paths.env_file)?;
            let home = steam::cmd_home(&paths.steamcmd_home, &steam::client_steamapps())?;
            return setup::login(&steamcmd, &home, &username);
        }
        Some(other) => anyhow::bail!("unknown command `{other}`\n\n{USAGE}"),
    }

    let config = steam::SteamConfig::from_env(&paths.env_file, paths.steamcmd_home.clone())?;
    if let Some(dir) = paths.db.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let db_path      = paths.db.clone();
    let db           = Database::open(&db_path)?;

    // ── Color palette ─────────────────────────
    // Prefer ~/.config/noguisteam/colors.json (XDG); fall back to an old
    // checkout's colors.json. Missing files use the built-in palette.
    let palette_path = xdg_config_path("colors.json")
        .filter(|p| p.exists())
        .or_else(|| paths.legacy.as_ref().map(|r| r.join("colors.json")))
        .unwrap_or_default();
    ui::set_palette(ui::load_palette_from(&palette_path));

    // One shared HTTP client (connection pool, gzip, timeouts) used for
    // every Steam/ITAD/CDN request in the app.
    let http = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent(concat!("noguisteam/", env!("CARGO_PKG_VERSION")))
        .build()?;

    let mut app  = App::new(&db, config.steamlib.clone())?;

    // Query the terminal's graphics protocol before raw mode / alt screen.
    let picker = ratatui_image::picker::Picker::from_query_stdio()
        .unwrap_or_else(|_| ratatui_image::picker::Picker::halfblocks());
    let _ = io::Write::flush(&mut io::stdout());
    let mut renderer = ImageRenderer::new(http.clone(), picker);

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

    let result = run(&mut term, &mut app, &db, &db_path, &config, &mut renderer, &http);

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
    db_path:  &Path,
    config:   &steam::SteamConfig,
    renderer: &mut ImageRenderer,
    http:     &reqwest::blocking::Client,
) -> anyhow::Result<()> {

    let (install_tx, install_rx)   = mpsc::channel::<InstallMsg>();
    let (wishlist_tx, wishlist_rx) = mpsc::channel::<Result<Vec<WishlistEntry>, String>>();
    let (sync_tx, sync_rx)         = mpsc::channel::<Result<usize, String>>();

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
            let db_p   = db_path.to_path_buf();
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
        let db_p   = db_path.to_path_buf();
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
        while let Ok(msg) = install_rx.try_recv() {
            if app.install_state.feed(msg) {
                let _ = app.reload_games(db);
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

        // ── Force full redraw if flagged ─────
        if app.needs_clear {
            app.needs_clear = false;
            term.clear()?;
        }

        // ── Draw TUI ─────────────────────────
        term.draw(|f| ui::draw(f, app, renderer))?;

        // ── Poll events ──────────────────────
        if !event::poll(Duration::from_millis(100))? {
            continue;
        }

        if let Event::Key(key) = event::read()? {
            // Help popup: any key closes it.
            if app.show_help {
                app.show_help = false;
                continue;
            }
            if key.code == KeyCode::Char('?')
                && !app.search_mode
                && !matches!(app.install_state, InstallState::Running { .. })
            {
                app.show_help = true;
                continue;
            }
            match key.code {
                KeyCode::Char('q') | KeyCode::Char('Q') if !app.search_mode => {
                    app.should_quit = true;
                }
                KeyCode::Tab => {
                    app.next_tab();
                }
                KeyCode::BackTab => {
                    app.prev_tab();
                }
                _ => {}
            }

            if app.should_quit { break; }

            match app.active_tab {
                Tab::Library  => handle_library(key, app, db_path, config, install_tx.clone(), sync_tx.clone(), http),
                Tab::Wishlist => handle_wishlist(key, app, config, db_path, http, wishlist_tx.clone()),
                Tab::Stats    => {}
            }
        }
    }

    Ok(())
}

// ─────────────────────────────────────────────
// Library key handling
// ─────────────────────────────────────────────
#[allow(clippy::too_many_arguments)]
fn handle_library(
    key:              crossterm::event::KeyEvent,
    app:              &mut App,
    db_path:          &Path,
    config:           &steam::SteamConfig,
    install_tx:       mpsc::Sender<InstallMsg>,
    sync_tx:          mpsc::Sender<Result<usize, String>>,
    http:             &reqwest::blocking::Client,
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
            app.move_up();
        }
        KeyCode::Down | KeyCode::Char('j') => {
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
                let db_p       = db_path.to_path_buf();
                let cfg_clone  = config.clone();
                let name       = game.name.clone();

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
                        Err(e) => { let _ = tx.send(InstallMsg::Finished(Err(format!("failed to open DB: {e}")))); return; }
                    };
                    let result = steam::install_game(app_id, &cfg_clone, &db2, tx.clone())
                        .map(|_| format!("{name} installed."))
                        .map_err(|e| e.to_string());
                    let _ = tx.send(InstallMsg::Finished(result));
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
                let db_p      = db_path.to_path_buf();
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
                        Err(e) => { let _ = tx.send(InstallMsg::Finished(Err(format!("failed to open DB: {e}")))); return; }
                    };
                    match steam::uninstall_game(app_id, &cfg_clone, &db2) {
                        Ok(_)  => { let _ = tx.send(InstallMsg::Finished(Ok(format!("{name} uninstalled.")))); }
                        Err(e) => { let _ = tx.send(InstallMsg::Finished(Err(e.to_string()))); }
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

            let db_p      = db_path.to_path_buf();
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
    db_path: &Path,
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
            let db_p     = db_path.to_path_buf();
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

fn sort_wishlist_entries(entries: &mut [app::WishlistEntry], sort: &WishlistSort) {
    match sort {
        WishlistSort::Deal     => entries.sort_by(|a, b| b.deal_score().total_cmp(&a.deal_score())
                                      .then(b.discount_percent.cmp(&a.discount_percent))),
        WishlistSort::Discount => entries.sort_by_key(|e| std::cmp::Reverse(e.discount_percent)),
        WishlistSort::Price    => entries.sort_by(|a, b| a.current_price.total_cmp(&b.current_price)),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, regular: f64, current: f64, pct: u32) -> app::WishlistEntry {
        app::WishlistEntry {
            name: name.into(), current_price: current, regular_price: regular,
            discount_percent: pct, deal_tag: String::new(), store_low: None,
            historical_low: None, url: String::new(),
        }
    }

    #[test]
    fn deal_sort_ranks_saving_weighted_by_discount() {
        let mut v = vec![
            entry("small markdown, pricey", 300.0, 240.0, 20), // saves 60 × 0.20 = 12
            entry("deep cut, cheap",         40.0,   8.0, 80), // saves 32 × 0.80 = 25.6
            entry("big saving, deep",       200.0,  50.0, 75), // saves 150 × 0.75 = 112.5
            entry("not on sale",            100.0, 100.0,  0), // 0
        ];
        sort_wishlist_entries(&mut v, &WishlistSort::Deal);
        let names: Vec<&str> = v.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["big saving, deep", "deep cut, cheap", "small markdown, pricey", "not on sale"]);
    }
}
