pub(crate) mod about;
mod chrome;
mod installed;
mod repos;
mod search;
mod settings;
mod updates;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};

use pike_core::package::SourceType;
use pike_core::util::truncate_str;
use rust_i18n::t;

use super::app::App;
use super::types::{HitState, Tab, TableClickZone, ViewState};

pub(super) const ACCENT: Color = Color::Rgb(254, 128, 25);
pub(super) const FG: Color = Color::White;
pub(super) const FG_DIM: Color = Color::Rgb(180, 180, 184);
pub(super) const FG_FAINT: Color = Color::Rgb(120, 120, 128);
pub(super) const FG_SUBTLE: Color = Color::Rgb(68, 68, 72);
pub(super) const SELECTED_BG: Color = Color::Rgb(254, 128, 25);
pub(super) const SELECTED_FG: Color = Color::Rgb(0, 0, 0);
pub(super) const GREEN: Color = Color::Rgb(50, 215, 75);
pub(super) const RED: Color = Color::Rgb(255, 69, 58);
pub(super) const HOVER_FG: Color = Color::Rgb(210, 210, 214);

pub(super) const TABLE_WIDTHS: [Constraint; 5] = [
    Constraint::Length(10),
    Constraint::Length(30),
    Constraint::Length(8),
    Constraint::Length(15),
    Constraint::Min(20),
];

pub(crate) fn render(frame: &mut Frame, app: &App, view: &mut ViewState, hit: &mut HitState) {
    let inner = frame.area().inner(Margin::new(2, 1));

    let [
        title_area,
        _title_gap,
        tab_area,
        sep_area,
        context_area,
        content_area,
        sep2_area,
        footer_area,
    ] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);

    chrome::render_title(frame, title_area);
    chrome::render_tab_bar(frame, app, hit, tab_area);

    frame.render_widget(
        Paragraph::new("─".repeat(sep_area.width as usize)).style(Style::default().fg(FG_SUBTLE)),
        sep_area,
    );

    chrome::render_context_bar(frame, app, context_area);

    match app.tab {
        Tab::Search => search::render_search(frame, app, view, hit, content_area),
        Tab::Installed => installed::render_installed(frame, app, view, hit, content_area),
        Tab::Updates => updates::render_updates(frame, app, view, hit, content_area),
        Tab::Repos => repos::render_repos(frame, app, view, hit, content_area),
        Tab::Settings => settings::render_settings(frame, app, view, hit, content_area),
        Tab::About => {
            let selected = view.about_table.selected().unwrap_or(0);
            about::render_about(frame, content_area, selected);
        }
    }

    frame.render_widget(
        Paragraph::new("─".repeat(sep2_area.width as usize)).style(Style::default().fg(FG_SUBTLE)),
        sep2_area,
    );

    chrome::render_footer(frame, app, view, hit, footer_area);
}

pub(super) fn borderless_block() -> Block<'static> {
    Block::default().borders(Borders::NONE)
}

pub(super) fn render_centered_empty(
    frame: &mut Frame,
    area: Rect,
    icon: &str,
    msg: &str,
    icon_color: Color,
) {
    if area.height < 3 {
        let line = Line::from(vec![
            Span::styled(format!("{icon} "), Style::default().fg(icon_color)),
            Span::styled(msg, Style::default().fg(FG_FAINT)),
        ]);
        frame.render_widget(Paragraph::new(line), area);
        return;
    }

    let center_y = area.y + area.height / 2;

    if center_y > area.y {
        let icon_area = Rect::new(area.x, center_y.saturating_sub(1), area.width, 1);
        frame.render_widget(
            Paragraph::new(Span::styled(icon, Style::default().fg(icon_color)))
                .alignment(Alignment::Center),
            icon_area,
        );
    }

    let msg_area = Rect::new(area.x, center_y, area.width, 1);
    frame.render_widget(
        Paragraph::new(Span::styled(msg, Style::default().fg(FG_FAINT)))
            .alignment(Alignment::Center),
        msg_area,
    );
}

pub(super) fn row_styles(hovered: bool, accent: Color) -> (Style, Style, Style, Style) {
    if hovered {
        let h = Style::default().fg(HOVER_FG);
        (h, h, h, h)
    } else {
        (
            Style::default().fg(FG_DIM),
            Style::default().fg(FG),
            Style::default().fg(FG_DIM),
            Style::default().fg(accent),
        )
    }
}

pub(super) struct PackageRowData<'a> {
    pub index: usize,
    pub hover: Option<usize>,
    pub source: String,
    pub name: &'a str,
    pub arch: Option<&'a str>,
    pub version: &'a str,
    pub description: Option<&'a str>,
    pub installed: bool,
}

pub(super) fn make_package_row<'a>(data: PackageRowData<'a>) -> Row<'a> {
    let hov = data.hover == Some(data.index);
    let desc = truncate_str(data.description.unwrap_or(""), 50).into_owned();
    let (src_style, name_style, ver_style, desc_style) = row_styles(hov, FG_FAINT);
    let arch_style = if hov {
        Style::default().fg(HOVER_FG)
    } else {
        Style::default().fg(FG_FAINT)
    };

    let name_cell = if data.installed {
        Cell::from(Line::from(vec![
            Span::styled("✓ ", Style::default().fg(GREEN)),
            Span::styled(data.name, name_style),
        ]))
    } else {
        Cell::from(data.name).style(name_style)
    };

    Row::new(vec![
        Cell::from(data.source).style(src_style),
        name_cell,
        Cell::from(data.arch.unwrap_or("-").to_string()).style(arch_style),
        Cell::from(data.version).style(ver_style),
        Cell::from(desc).style(desc_style),
    ])
}

pub(super) fn package_header() -> Row<'static> {
    Row::new([
        t!("header.source").to_string(),
        t!("header.name").to_string(),
        t!("header.arch").to_string(),
        t!("header.version").to_string(),
        t!("header.description").to_string(),
    ])
    .style(Style::default().fg(FG_FAINT))
    .bottom_margin(1)
}

pub(super) fn render_filter_input(frame: &mut Frame, filter_text: &str, editing: bool, area: Rect) {
    let prompt_style = if editing {
        Style::default().fg(ACCENT)
    } else {
        Style::default().fg(FG_FAINT)
    };

    let line = Line::from(vec![
        Span::styled("/ ", prompt_style),
        Span::styled(filter_text, Style::default().fg(FG)),
    ]);
    frame.render_widget(Paragraph::new(line), area);

    if editing {
        frame.set_cursor_position((
            area.x + 2 + unicode_width::UnicodeWidthStr::width(filter_text) as u16,
            area.y,
        ));
    }
}

pub(super) fn split_filter_area(
    frame: &mut Frame,
    filter_text: &str,
    editing: bool,
    area: Rect,
) -> Rect {
    let has_filter = !filter_text.is_empty() || editing;
    if !has_filter {
        return area;
    }

    let [filter_area, _gap, table_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
    ])
    .areas(area);

    render_filter_input(frame, filter_text, editing, filter_area);
    table_area
}

pub(super) fn format_count_line<T>(
    items: &[T],
    shown: usize,
    source_of: impl Fn(&T) -> SourceType,
    noun: &str,
) -> Line<'static> {
    let total = items.len();
    let text = if shown < total {
        t!(
            "tui.context.filtered",
            shown = shown,
            total = total,
            noun = noun
        )
        .to_string()
    } else {
        let parts: Vec<String> = SourceType::ALL
            .iter()
            .filter_map(|&st| {
                let c = items.iter().filter(|item| source_of(item) == st).count();
                if c > 0 {
                    Some(format!("{c} {st}"))
                } else {
                    None
                }
            })
            .collect();
        if parts.is_empty() {
            format!("{total} {noun}")
        } else {
            format!("{total} {noun} ({})", parts.join(" \u{00b7} "))
        }
    };
    Line::from(Span::styled(text, Style::default().fg(FG_FAINT)))
}

pub(super) fn render_table_widget(
    frame: &mut Frame,
    hit: &mut HitState,
    state: &mut ratatui::widgets::TableState,
    area: Rect,
    header: Row,
    rows: Vec<Row>,
    widths: &[Constraint],
) {
    let data_y = area.y + 2;
    let data_height = area.height.saturating_sub(2);
    hit.table_zone = Some(TableClickZone {
        y_start: data_y,
        x_start: area.x,
        width: area.width,
        visible_rows: data_height,
        item_count: rows.len(),
    });
    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(Style::default().bg(SELECTED_BG).fg(SELECTED_FG))
        .block(borderless_block());
    frame.render_stateful_widget(table, area, state);
}
