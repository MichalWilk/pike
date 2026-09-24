use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::widgets::{Cell, Paragraph, Row};
use rust_i18n::t;

use pike_core::package::{CleanupItem, CleanupKind};

use crate::format::{cleanup_version, format_size};
use crate::tui::app::App;
use crate::tui::types::{CHECKBOX_WIDTH, HitState, Tab, ViewState};

use super::{
    ACCENT, FG_DIM, FG_FAINT, GREEN, HOVER_FG, render_centered_empty, render_table_widget,
    row_styles, split_filter_area,
};

fn reason_label(kind: CleanupKind) -> String {
    match kind {
        CleanupKind::Orphan => t!("tui.cleanup.reason-orphan"),
        CleanupKind::UnusedRuntime => t!("tui.cleanup.reason-runtime"),
        CleanupKind::OldKernel => t!("tui.cleanup.reason-kernel"),
        CleanupKind::Cache => t!("tui.cleanup.reason-cache"),
    }
    .to_string()
}

fn reason_detail(item: &CleanupItem, keep_kernels: usize) -> String {
    let name = item.name.as_str();
    let version = cleanup_version(item);
    let version = version.as_str();
    match item.kind {
        CleanupKind::Orphan => t!("tui.cleanup.detail-orphan", name = name),
        CleanupKind::UnusedRuntime => {
            t!("tui.cleanup.detail-runtime", name = name, version = version)
        }
        CleanupKind::OldKernel => {
            t!(
                "tui.cleanup.detail-kernel",
                version = version,
                keep = keep_kernels
            )
        }
        CleanupKind::Cache => t!("tui.cleanup.detail-cache", name = name),
    }
    .to_string()
}

fn render_detail(frame: &mut Frame, app: &App, view: &ViewState, filtered: &[usize], area: Rect) {
    let highlighted = view
        .cleanup_table
        .selected()
        .and_then(|s| filtered.get(s))
        .and_then(|&i| app.cleanup.items.get(i));
    if let Some(item) = highlighted {
        let detail = reason_detail(item, app.config.cleanup.keep_kernels());
        frame.render_widget(
            Paragraph::new(detail).style(Style::default().fg(FG_DIM)),
            area,
        );
    }
}

pub(super) fn render_cleanup(
    frame: &mut Frame,
    app: &App,
    view: &mut ViewState,
    hit: &mut HitState,
    area: Rect,
) {
    if app.cleanup.loading {
        let msg = t!("tui.cleanup.scanning");
        render_centered_empty(frame, area, &view.spinner_char().to_string(), &msg, ACCENT);
        return;
    }

    if app.cleanup.items.is_empty() {
        let msg = t!("tui.cleanup.nothing");
        render_centered_empty(frame, area, "✓", &msg, GREEN);
        return;
    }

    let editing = app.is_editing_on(Tab::Cleanup);
    let table_area = split_filter_area(frame, &app.cleanup.filter, editing, area);

    let filtered = app.cleanup_filtered_indices();
    if filtered.is_empty() {
        let msg = t!("tui.cleanup.no-match");
        render_centered_empty(frame, table_area, "∅", &msg, FG_FAINT);
        return;
    }

    let table_area = if table_area.height >= 6 {
        let [table_area, _gap, detail_area] = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(table_area);
        render_detail(frame, app, view, &filtered, detail_area);
        table_area
    } else {
        table_area
    };

    let hover = view.hover_row;
    let header = Row::new([
        String::new(),
        t!("header.source").to_string(),
        t!("header.name").to_string(),
        t!("header.version").to_string(),
        t!("header.size").to_string(),
        t!("header.reason").to_string(),
    ])
    .style(Style::default().fg(FG_FAINT))
    .bottom_margin(1);

    let rows: Vec<Row> = filtered
        .iter()
        .enumerate()
        .map(|(vi, &real_i)| {
            let item = &app.cleanup.items[real_i];
            let hov = hover == Some(vi);
            let (source_style, name_style, ver_style, size_style) = row_styles(hov, ACCENT);
            let reason_style = if hov {
                Style::default().fg(HOVER_FG)
            } else {
                Style::default().fg(FG_FAINT)
            };
            let check = if app.cleanup_selected.contains(&real_i) {
                "[x]"
            } else {
                "[ ]"
            };
            Row::new(vec![
                Cell::from(check).style(name_style),
                Cell::from(item.source.to_string()).style(source_style),
                Cell::from(item.name.as_str()).style(name_style),
                Cell::from(cleanup_version(item)).style(ver_style),
                Cell::from(item.size.map(format_size).unwrap_or_default()).style(size_style),
                Cell::from(reason_label(item.kind)).style(reason_style),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(CHECKBOX_WIDTH),
        Constraint::Length(8),
        Constraint::Length(28),
        Constraint::Length(18),
        Constraint::Length(10),
        Constraint::Min(14),
    ];

    render_table_widget(
        frame,
        hit,
        &mut view.cleanup_table,
        table_area,
        header,
        rows,
        &widths,
    );
}
