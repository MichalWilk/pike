use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::widgets::Row;
use rust_i18n::t;

use crate::tui::app::App;
use crate::tui::types::{HitState, Tab, ViewState};

use super::{
    ACCENT, FG_FAINT, PackageRowData, TABLE_WIDTHS, make_package_row, package_header,
    render_centered_empty, render_table_widget, split_filter_area,
};

pub(super) fn render_installed(
    frame: &mut Frame,
    app: &App,
    view: &mut ViewState,
    hit: &mut HitState,
    area: Rect,
) {
    if app.installed.loading {
        let msg = t!("tui.installed.loading");
        render_centered_empty(frame, area, &view.spinner_char().to_string(), &msg, ACCENT);
        return;
    }

    if app.installed.items.is_empty() {
        let msg = t!("tui.installed.empty");
        render_centered_empty(frame, area, "-", &msg, FG_FAINT);
        return;
    }

    let editing = app.is_editing_on(Tab::Installed);
    let table_area = split_filter_area(frame, &app.installed.filter, editing, area);

    let filtered = app.installed_filtered_indices();

    if filtered.is_empty() {
        let msg = t!("tui.installed.no-match");
        render_centered_empty(frame, table_area, "∅", &msg, FG_FAINT);
        return;
    }

    let hover = view.hover_row;
    let header = package_header();

    let rows: Vec<Row> = filtered
        .iter()
        .enumerate()
        .map(|(vi, &real_i)| {
            let pkg = &app.installed.items[real_i];
            make_package_row(PackageRowData {
                index: vi,
                hover,
                source: pkg.source.to_string(),
                name: &pkg.name,
                arch: pkg.arch.as_deref(),
                version: &pkg.version,
                description: pkg.description.as_deref(),
                installed: false,
            })
        })
        .collect();

    render_table_widget(
        frame,
        hit,
        &mut view.installed_table,
        table_area,
        header,
        rows,
        &TABLE_WIDTHS,
    );
}
