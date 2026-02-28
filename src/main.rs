// main.rs — Entry point, event loop

mod app;
mod db;
mod image;
mod steam;
mod ui;

use app::{App, InstallState, Tab, WishlistSort};
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
    dotenv::dotenv().ok();
    let config = steam::SteamConfig::from_env()?;

    let project_root = project_root();
    let db_path      = project_root.join("steam_games.db");
    let db           = Database::open(&db_path)?;

    let mut app  = App::new(&db)?;
    let renderer = ImageRenderer::new();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend  = CrosstermBackend::new(stdout);
    let mut term = Terminal::new(backend)?;

    let result = run(&mut term, &mut app, &db, &db_path, &config, &renderer);

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
) -> anyhow::Result<()> {

    let (install_tx, install_rx) = mpsc::channel::<String>();
    let mut last_rendered_appid: Option<u32> = None;

    loop {
        // ── Drain install subprocess output ──
        while let Ok(line) = install_rx.try_recv() {
            if let InstallState::Running { output, app_id } = &mut app.install_state {
                if line.starts_with("✅") {
                    let id = *app_id;
                    app.install_state = InstallState::Done { app_id: id, success: true };
                    let _ = app.reload_games(db);
                } else if line.to_lowercase().contains("error") || line.to_lowercase().contains("failed") {
                    let id = *app_id;
                    app.install_state = InstallState::Done { app_id: id, success: false };
                } else {
                    output.push(line);
                }
            }
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
                    app.next_tab();
                }
                KeyCode::BackTab => {
                    app.prev_tab();
                }
                _ => {}
            }

            if app.should_quit { break; }

            match app.active_tab {
                Tab::Library  => handle_library(key, app, db, db_path, config, install_tx.clone(), &renderer, &mut last_rendered_appid),
                Tab::Wishlist => handle_wishlist(key, app, config),
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
    db:               &Database,
    db_path:          &PathBuf,
    config:           &steam::SteamConfig,
    install_tx:       mpsc::Sender<String>,
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
                let app_id = game.app_id;
                let tx     = install_tx.clone();
                let db_p   = db_path.clone();

                app.install_state = InstallState::Running {
                    app_id,
                    output: vec![format!("Starting install for {}…", game.name)],
                };

                thread::spawn(move || {
                    let cfg = steam::SteamConfig::from_env().unwrap();
                    let db  = Database::open(&db_p).unwrap();
                    let _   = steam::install_game(app_id, &cfg, &db, tx);
                });
            }
        }
        KeyCode::Char('u') | KeyCode::Char('U') => {
            if let Some(game) = app.selected_game().cloned() {
                if !game.installed {
                    app.set_status(format!("{} is not installed.", game.name));
                    return;
                }
                match steam::uninstall_game(game.app_id, config, db) {
                    Ok(_)  => { let _ = app.reload_games(db); app.set_status(format!("{} uninstalled.", game.name)); }
                    Err(e) => app.set_status(format!("Uninstall failed: {}", e)),
                }
            }
        }
        KeyCode::Char('p') | KeyCode::Char('P') => {
            if let Some(game) = app.selected_game() {
                let _ = std::process::Command::new("steam")
                    .arg(format!("steam://run/{}", game.app_id))
                    .spawn();
                app.set_status(format!("Launching {}…", game.name));
            }
        }
        KeyCode::Char('l') | KeyCode::Char('L') => {
            app.set_status("Syncing library…".to_string());
            let db_p = db_path.clone();
            let handle = thread::spawn(move || {
                let cfg = steam::SteamConfig::from_env().unwrap();
                steam::sync_library_to(&cfg, &db_p)
            });
            match handle.join().unwrap() {
                Ok(n)  => { let _ = app.reload_games(db); app.set_status(format!("Synced — {} games.", n)); }
                Err(e) => app.set_status(format!("Sync failed: {}", e)),
            }
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
fn handle_wishlist(key: crossterm::event::KeyEvent, app: &mut App, config: &steam::SteamConfig) {
    match key.code {
        KeyCode::Up   | KeyCode::Char('k') => { if app.wishlist_sel > 0 { app.wishlist_sel -= 1; } }
        KeyCode::Down | KeyCode::Char('j') => { if app.wishlist_sel + 1 < app.wishlist.len() { app.wishlist_sel += 1; } }
        KeyCode::Char('s') | KeyCode::Char('S') => {
            app.wishlist_sort = match app.wishlist_sort {
                WishlistSort::Deal     => WishlistSort::Discount,
                WishlistSort::Discount => WishlistSort::Price,
                WishlistSort::Price    => WishlistSort::Deal,
            };
            sort_wishlist(app);
        }
        KeyCode::Char('r') | KeyCode::Char('R') => {
            app.wishlist_loading = true;
            app.set_status("Fetching wishlist…".to_string());
            match steam::fetch_wishlist_sales(config) {
                Ok(mut entries) => {
                    sort_wishlist_entries(&mut entries, &app.wishlist_sort);
                    app.wishlist         = entries;
                    app.wishlist_sel     = 0;
                    app.wishlist_loading = false;
                    app.set_status(format!("{} sales found.", app.wishlist.len()));
                }
                Err(e) => {
                    app.wishlist_loading = false;
                    app.set_status(format!("Error: {}", e));
                }
            }
        }
        _ => {}
    }
}

fn sort_wishlist(app: &mut App) {
    sort_wishlist_entries(&mut app.wishlist, &app.wishlist_sort);
    app.wishlist_sel = 0;
}

fn sort_wishlist_entries(entries: &mut Vec<app::WishlistEntry>, sort: &WishlistSort) {
    match sort {
        WishlistSort::Deal     => entries.sort_by(|a, b| deal_order(&a.deal_tag).cmp(&deal_order(&b.deal_tag)).then(a.current_price.partial_cmp(&b.current_price).unwrap())),
        WishlistSort::Discount => entries.sort_by(|a, b| b.discount_percent.cmp(&a.discount_percent)),
        WishlistSort::Price    => entries.sort_by(|a, b| a.current_price.partial_cmp(&b.current_price).unwrap()),
    }
}

fn deal_order(tag: &str) -> u8 {
    if tag.contains("All-Time") { 0 } else if tag.contains("Cross") { 1 } else if tag.contains("Match") { 2 } else { 3 }
}

// ─────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────
fn project_root() -> PathBuf {
    if let Ok(p) = std::env::var("NOGUISTEAM_HOME") {
        return PathBuf::from(p);
    }
    // fallback: walk up from binary
    let exe = std::fs::canonicalize(std::env::current_exe().unwrap_or_default())
        .unwrap_or_default();
    let mut dir = exe.parent().unwrap_or(std::path::Path::new(".")).to_path_buf();
    for _ in 0..6 {
        if dir.join(".env").exists() || dir.join("steam_games.db").exists() {
            return dir;
        }
        if let Some(p) = dir.parent() { dir = p.to_path_buf(); } else { break; }
    }
    std::env::current_dir().unwrap_or_default()
}
