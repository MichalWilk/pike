use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::Style;
use ratatui::widgets::{Cell, Row};
use rust_i18n::t;

use crate::tui::app::App;
use crate::tui::types::{HitState, Tab, ViewState};

use super::{
    ACCENT, FG_FAINT, GREEN, HOVER_FG, render_centered_empty, render_table_widget, row_styles,
    split_filter_area,
};

pub(super) fn render_updates(
    frame: &mut Frame,
    app: &App,
    view: &mut ViewState,
    hit: &mut HitState,
    area: Rect,
) {
    if app.updates.loading {
        let msg = t!("tui.updates.checking");
        render_centered_empty(frame, area, &view.spinner_char().to_string(), &msg, ACCENT);
        return;
    }

    if app.updates.items.is_empty() {
        let msg = t!("tui.updates.up-to-date");
        render_centered_empty(frame, area, "✓", &msg, GREEN);
        return;
    }

    let editing = app.is_editing_on(Tab::Updates);
    let table_area = split_filter_area(frame, &app.updates.filter, editing, area);

    let filtered = app.updates_filtered_indices();
    if filtered.is_empty() {
        let msg = t!("tui.updates.no-match");
        render_centered_empty(frame, table_area, "∅", &msg, FG_FAINT);
        return;
    }

    let hover = view.hover_row;

    let header = Row::new([
        t!("header.source").to_string(),
        t!("header.name").to_string(),
        t!("header.arch").to_string(),
        t!("header.installed-ver").to_string(),
        t!("header.available").to_string(),
    ])
    .style(Style::default().fg(FG_FAINT))
    .bottom_margin(1);

    let rows: Vec<Row> = filtered
        .iter()
        .enumerate()
        .map(|(vi, &real_i)| {
            let u = &app.updates.items[real_i];
            let hov = hover == Some(vi);
            let (source_style, name_style, ver_style, avail_style) = row_styles(hov, GREEN);
            let arch_style = if hov {
                Style::default().fg(HOVER_FG)
            } else {
                Style::default().fg(FG_FAINT)
            };
            let installed = if u.installed_version.is_empty() {
                "-"
            } else {
                &u.installed_version
            };
            Row::new(vec![
                Cell::from(u.source.to_string()).style(source_style),
                Cell::from(u.name.as_str()).style(name_style),
                Cell::from(u.arch.as_deref().unwrap_or("-").to_string()).style(arch_style),
                Cell::from(installed).style(ver_style),
                Cell::from(format!("→ {}", u.available_version)).style(avail_style),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(10),
        Constraint::Length(30),
        Constraint::Length(8),
        Constraint::Length(18),
        Constraint::Min(18),
    ];

    render_table_widget(
        frame,
        hit,
        &mut view.updates_table,
        table_area,
        header,
        rows,
        &widths,
    );
}
