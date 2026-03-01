// ui.rs — Ratatui rendering

use crate::app::{App, InstallState, Tab, WishlistSort};
use crate::db::Game;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Bar, BarChart, BarGroup, Block, Borders, Cell, Clear,
        List, ListItem, ListState, Paragraph, Row, Table, Tabs, Wrap,
    },
    Frame,
};

// ── Palette ──────────────────────────────────
const ACCENT:   Color = Color::Cyan;
const DIM:      Color = Color::DarkGray;
const OK:       Color = Color::Green;
const WARN:     Color = Color::Yellow;
const ERR:      Color = Color::Red;
const SEL:      Color = Color::Cyan;

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.size();

    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);

    draw_tabs(f, app, root[0]);

    match app.active_tab {
        Tab::Library  => draw_library(f, app, root[1]),
        Tab::Wishlist => draw_wishlist(f, app, root[1]),
        Tab::Stats    => draw_stats(f, app, root[1]),
    }

    draw_status(f, app, root[2]);

    if matches!(&app.install_state, InstallState::Running { .. } | InstallState::Done { .. }) {
        draw_install_popup(f, app, area);
    }
}

// ─────────────────────────────────────────────
// Tab bar
// ─────────────────────────────────────────────
fn draw_tabs(f: &mut Frame, app: &App, area: Rect) {
    let titles: Vec<Line> = Tab::titles().iter().map(|t| Line::from(*t)).collect();
    let tabs = Tabs::new(titles)
        .select(app.active_tab.index())
        .block(Block::default().borders(Borders::BOTTOM))
        .highlight_style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD))
        .divider("|")
        .style(Style::default().fg(DIM));
    f.render_widget(tabs, area);
}

// ─────────────────────────────────────────────
// Status bar
// ─────────────────────────────────────────────
fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let keybinds: &str = match app.active_tab {
        Tab::Library if app.search_mode =>
            " ESC: Cancel search  |  Type to filter",
        Tab::Library =>
            " /: Search  |  I: Install  |  U: Uninstall  |  P: Play  |  L: Sync  |  Tab: Switch  |  Q: Quit",
        Tab::Wishlist =>
            " R: Refresh  |  S: Sort (Deal/Discount/Price)  |  O: Open in browser  |  Tab: Switch  |  Q: Quit",
        Tab::Stats =>
            " Tab: Switch  |  Q: Quit",
    };

    let (msg, color): (&str, Color) = match &app.status_msg {
        Some(s) => (s.as_str(), OK),
        None    => (keybinds, DIM),
    };

    f.render_widget(Paragraph::new(msg).style(Style::default().fg(color)), area);
}

// ─────────────────────────────────────────────
// Library tab
// ─────────────────────────────────────────────
fn draw_library(f: &mut Frame, app: &App, area: Rect) {
    // Detail panel is fixed at 48 cols (46 image cols + 2 border), matching 460px @ 10px/cell
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(48)])
        .split(area);

    draw_game_list(f, app, chunks[0]);
    draw_game_detail(f, app, chunks[1]);
}

fn draw_game_list(f: &mut Frame, app: &App, area: Rect) {
    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(area);

    // Search box
    let search_text: String = if app.search_mode {
        format!("🔍 {}_", app.search)
    } else if app.search.is_empty() {
        " Search...".into()
    } else {
        format!("🔍 {}", app.search)
    };
    let search_style = if app.search_mode {
        Style::default().fg(ACCENT)
    } else {
        Style::default().fg(DIM)
    };
    f.render_widget(
        Paragraph::new(search_text)
            .style(search_style)
            .block(Block::default().borders(Borders::ALL).title(" Filter ")),
        inner[0],
    );

    // Game list
    let items: Vec<ListItem> = app.filtered.iter().map(|&i| {
        let g = &app.games[i];
        let (icon, icon_color) = if g.installed { ("● ", OK) } else { ("○ ", DIM) };
        ListItem::new(Line::from(vec![
            Span::styled(icon, Style::default().fg(icon_color)),
            Span::raw(g.name.as_str()),
        ]))
    }).collect();

    let title = if app.filtered.len() == app.games.len() {
        format!(" Games ({}) ", app.games.len())
    } else {
        format!(" Games ({}/{}) ", app.filtered.len(), app.games.len())
    };

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(Style::default().fg(SEL).add_modifier(Modifier::BOLD))
        .highlight_symbol("▶ ");

    let mut state = ListState::default();
    state.select(if app.filtered.is_empty() { None } else { Some(app.selected) });
    f.render_stateful_widget(list, inner[1], &mut state);
}

fn draw_game_detail(f: &mut Frame, app: &App, area: Rect) {
    // Cover block: 11 rows = 9 image cells + 2 border rows (460x215px @ 10x23px/cell)
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(11), Constraint::Min(0)])
        .split(area);

    // Draw the block border so the image has a visual container
    f.render_widget(
        Block::default().borders(Borders::ALL).title(" Cover ").style(Style::default().fg(DIM)),
        chunks[0],
    );

    match app.selected_game() {
        None => {
            f.render_widget(
                Paragraph::new("No game selected.")
                    .style(Style::default().fg(DIM))
                    .block(Block::default().borders(Borders::ALL)),
                chunks[1],
            );
        }
        Some(g) => {
            let rows = vec![
                Row::new(vec![
                    Cell::from("AppID").style(Style::default().fg(DIM)),
                    Cell::from(g.app_id.to_string()),
                ]),
                Row::new(vec![
                    Cell::from("Playtime").style(Style::default().fg(DIM)),
                    Cell::from(g.playtime_fmt()),
                ]),
                Row::new(vec![
                    Cell::from("Last played").style(Style::default().fg(DIM)),
                    Cell::from(format_timestamp(g.last_played)),
                ]),
                Row::new(vec![
                    Cell::from("Status").style(Style::default().fg(DIM)),
                    if g.installed {
                        Cell::from(Span::styled("✅ Installed",     Style::default().fg(OK)))
                    } else {
                        Cell::from(Span::styled("○  Not installed", Style::default().fg(DIM)))
                    },
                ]),
            ];

            let table = Table::new(rows, [Constraint::Length(14), Constraint::Min(0)])
                .block(Block::default().borders(Borders::ALL).title(format!(" {} ", g.name)));
            f.render_widget(table, chunks[1]);
        }
    }
}

// ─────────────────────────────────────────────
// Wishlist tab
// ─────────────────────────────────────────────
fn draw_wishlist(f: &mut Frame, app: &App, area: Rect) {
    if app.wishlist_loading {
        f.render_widget(
            Paragraph::new("⏳ Fetching wishlist sales…")
                .alignment(Alignment::Center)
                .style(Style::default().fg(ACCENT))
                .block(Block::default().borders(Borders::ALL).title(" Wishlist ")),
            area,
        );
        return;
    }

    if app.wishlist.is_empty() {
        f.render_widget(
            Paragraph::new("Press R to fetch wishlist sales")
                .alignment(Alignment::Center)
                .style(Style::default().fg(DIM))
                .block(Block::default().borders(Borders::ALL).title(" Wishlist ")),
            area,
        );
        return;
    }

    let sort_label = match app.wishlist_sort {
        WishlistSort::Deal     => "Deal",
        WishlistSort::Discount => "Discount",
        WishlistSort::Price    => "Price",
    };

    let sym = currency_symbol(
        &std::env::var("COUNTRY").unwrap_or_else(|_| "US".into())
    );

    let header = Row::new(vec!["Game", "Deal", "Price", "Regular", "Discount", "Steam Low", "Historic Low"])
        .style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD));

    let rows: Vec<Row> = app.wishlist.iter().map(|e| {
        Row::new(vec![
            Cell::from(e.name.as_str()),
            Cell::from(Span::styled(e.deal_tag.as_str(), Style::default().fg(deal_color(&e.deal_tag)))),
            Cell::from(format!("{}{:.2}", sym, e.current_price)),
            Cell::from(format!("{}{:.2}", sym, e.regular_price)).style(Style::default().fg(DIM)),
            Cell::from(format!("-{}%", e.discount_percent)).style(Style::default().fg(WARN)),
            Cell::from(e.store_low.map_or_else(|| "N/A".into(), |v| format!("{}{:.2}", sym, v))).style(Style::default().fg(DIM)),
            Cell::from(e.historical_low.map_or_else(|| "N/A".into(), |v| format!("{}{:.2}", sym, v))).style(Style::default().fg(DIM)),
        ])
    }).collect();

    let table = Table::new(rows, [
        Constraint::Percentage(28),
        Constraint::Percentage(22),
        Constraint::Percentage(8),
        Constraint::Percentage(8),
        Constraint::Percentage(8),
        Constraint::Percentage(10),
        Constraint::Percentage(10),
    ])
    .header(header)
    .block(Block::default().borders(Borders::ALL)
        .title(format!(" Wishlist Sales — sorted by {} ({} on sale) ", sort_label, app.wishlist.len())))
    .highlight_style(Style::default().fg(SEL).add_modifier(Modifier::BOLD))
    .highlight_symbol("▶ ");

    let mut state = ratatui::widgets::TableState::default();
    state.select(if app.wishlist.is_empty() { None } else { Some(app.wishlist_sel) });
    f.render_stateful_widget(table, area, &mut state);
}

// ─────────────────────────────────────────────
// Stats tab
// ─────────────────────────────────────────────
pub fn draw_stats(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(5), Constraint::Min(0)])
        .split(area);

    draw_stat_cards(f, app, chunks[0]);
    draw_stat_charts(f, app, chunks[1]);
}

fn draw_stat_cards(f: &mut Frame, app: &App, area: Rect) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(33), Constraint::Percentage(33), Constraint::Percentage(34)])
        .split(area);

    let total     = app.games.len();
    let installed = app.games.iter().filter(|g| g.installed).count();
    let total_h   = app.games.iter().map(|g| g.playtime_mins as u64).sum::<u64>() / 60;

    let cards: [(&str, String, Color); 3] = [
        (" Total Games ",    total.to_string(),          ACCENT),
        (" Installed ",      installed.to_string(),       OK),
        (" Total Playtime ", format!("{}h", total_h),    WARN),
    ];

    for (i, (title, value, color)) in cards.iter().enumerate() {
        f.render_widget(
            Paragraph::new(value.as_str())
                .alignment(Alignment::Center)
                .style(Style::default().fg(*color).add_modifier(Modifier::BOLD))
                .block(Block::default().borders(Borders::ALL).title(*title)),
            cols[i],
        );
    }
}

fn draw_stat_charts(f: &mut Frame, app: &App, area: Rect) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(area);

    // Top 10 most played bar chart
    let mut top: Vec<&Game> = app.games.iter().collect();
    top.sort_by(|a, b| b.playtime_mins.cmp(&a.playtime_mins));
    top.truncate(10);

    let bar_data: Vec<Bar> = top.iter().map(|g| {
        let label = if g.name.len() > 14 { format!("{}…", &g.name[..13]) } else { g.name.clone() };
        Bar::default()
            .value(g.playtime_mins as u64 / 60)
            .label(Line::from(label))
            .style(Style::default().fg(ACCENT))
    }).collect();

    f.render_widget(
        BarChart::default()
            .block(Block::default().borders(Borders::ALL).title(" Top 10 by Playtime (hours) "))
            .data(BarGroup::default().bars(&bar_data))
            .bar_width(3)
            .bar_gap(1)
            .value_style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD))
            .label_style(Style::default().fg(DIM)),
        cols[0],
    );

    // Recently played list
    let mut recent: Vec<&Game> = app.games.iter().filter(|g| g.last_played > 0).collect();
    recent.sort_by(|a, b| b.last_played.cmp(&a.last_played));
    recent.truncate(15);

    let items: Vec<ListItem> = recent.iter().map(|g| {
        ListItem::new(Line::from(vec![
            Span::styled("▸ ", Style::default().fg(ACCENT)),
            Span::raw(g.name.as_str()),
        ]))
    }).collect();

    f.render_widget(
        List::new(items)
            .block(Block::default().borders(Borders::ALL).title(" Recently Played "))
            .style(Style::default().fg(Color::White)),
        cols[1],
    );
}

// ─────────────────────────────────────────────
// Install progress popup
// ─────────────────────────────────────────────
fn draw_install_popup(f: &mut Frame, app: &App, area: Rect) {
    let popup = centered_rect(70, 60, area);
    f.render_widget(Clear, popup);

    let (title, lines, border_color) = match &app.install_state {
        InstallState::Running { app_id, output } => (
            format!(" Installing AppID {} ", app_id),
            output.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            ACCENT,
        ),
        InstallState::Done { app_id, success } => (
            format!(" AppID {} — {} ", app_id, if *success { "Done ✅" } else { "Failed ❌" }),
            vec!["Press any key to close."],
            if *success { OK } else { ERR },
        ),
        InstallState::Idle => return,
    };

    let inner_height = popup.height.saturating_sub(2) as usize;
    let display: Vec<Line> = lines.iter()
        .rev().take(inner_height).rev()
        .map(|l| Line::from(*l))
        .collect();

    f.render_widget(
        Paragraph::new(display)
            .wrap(Wrap { trim: true })
            .block(Block::default().borders(Borders::ALL).title(title)
                .border_style(Style::default().fg(border_color))),
        popup,
    );
}

// ─────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────
fn centered_rect(pct_x: u16, pct_y: u16, r: Rect) -> Rect {
    let vert = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - pct_y) / 2),
            Constraint::Percentage(pct_y),
            Constraint::Percentage((100 - pct_y) / 2),
        ])
        .split(r);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - pct_x) / 2),
            Constraint::Percentage(pct_x),
            Constraint::Percentage((100 - pct_x) / 2),
        ])
        .split(vert[1])[1]
}

fn format_timestamp(ts: u64) -> String {
    if ts == 0 { return "Never".into(); }
    let mut rem = ts;
    let days = rem / 86400; rem %= 86400;
    let hours = rem / 3600; rem %= 3600;
    let mins = rem / 60;
    let mut year = 1970u64;
    let mut d = days;
    loop {
        let yd = if is_leap(year) { 366 } else { 365 };
        if d < yd { break; }
        d -= yd; year += 1;
    }
    let mdays: [u64; 12] = [31, if is_leap(year) { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut month = 1u64;
    for m in mdays {
        if d < m { break; }
        d -= m; month += 1;
    }
    format!("{:04}-{:02}-{:02} {:02}:{:02}", year, month, d + 1, hours, mins)
}

fn is_leap(y: u64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn deal_color(tag: &str) -> Color {
    if tag.contains("All-Time")   { ERR  }
    else if tag.contains("Cross") { Color::LightRed }
    else if tag.contains("Match") { WARN }
    else                          { OK   }
}

fn currency_symbol(country: &str) -> &'static str {
    match country.to_uppercase().as_str() {
        "BR"       => "R$",
        "GB"       => "£",
        "DE" | "FR"=> "€",
        "AU"       => "A$",
        _          => "$",
    }
}
