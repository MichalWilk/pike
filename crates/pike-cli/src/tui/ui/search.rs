use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Row};
use rust_i18n::t;

use crate::tui::app::App;
use crate::tui::types::{HitState, Tab, ViewState};

use super::{
    ACCENT, FG_FAINT, PackageRowData, TABLE_WIDTHS, make_package_row, package_header,
    render_centered_empty, render_filter_input, render_table_widget,
};

pub(super) fn render_search(
    frame: &mut Frame,
    app: &App,
    view: &mut ViewState,
    hit: &mut HitState,
    area: Rect,
) {
    let [input_area, _gap, table_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
    ])
    .areas(area);

    render_search_input(frame, app, input_area);
    render_search_results(frame, app, view, hit, table_area);
}

fn render_search_input(frame: &mut Frame, app: &App, area: Rect) {
    let editing = app.is_editing_on(Tab::Search);
    if app.search.input.is_empty() && !editing {
        let placeholder = t!("tui.search.placeholder");
        let line = Line::from(vec![
            Span::styled("/ ", Style::default().fg(FG_FAINT)),
            Span::styled(placeholder.to_string(), Style::default().fg(FG_FAINT)),
        ]);
        frame.render_widget(Paragraph::new(line), area);
        return;
    }
    render_filter_input(frame, &app.search.input, editing, area);
}

fn render_search_results(
    frame: &mut Frame,
    app: &App,
    view: &mut ViewState,
    hit: &mut HitState,
    area: Rect,
) {
    if app.search.results.loading {
        let msg = t!("tui.search.searching");
        render_centered_empty(frame, area, &view.spinner_char().to_string(), &msg, ACCENT);
        return;
    }

    if app.search.results.items.is_empty() {
        if app.search.input.is_empty() {
            let msg = t!("tui.search.empty-hint");
            render_centered_empty(frame, area, "/", &msg, FG_FAINT);
        } else {
            let msg = t!("tui.search.no-results");
            render_centered_empty(frame, area, "∅", &msg, FG_FAINT);
        }
        return;
    }

    let filtered = app.search_filtered_indices();
    if filtered.is_empty() {
        let msg = t!("tui.search.no-match");
        render_centered_empty(frame, area, "∅", &msg, FG_FAINT);
        return;
    }

    let hover = view.hover_row;

    let header = package_header();

    let rows: Vec<Row> = filtered
        .iter()
        .enumerate()
        .map(|(vi, &real_i)| {
            let pkg = &app.search.results.items[real_i];
            let installed = app.is_installed(&pkg.name, pkg.source);
            make_package_row(PackageRowData {
                index: vi,
                hover,
                source: pkg.source.to_string(),
                name: &pkg.name,
                arch: pkg.arch.as_deref(),
                version: &pkg.version,
                description: pkg.description.as_deref(),
                installed,
            })
        })
        .collect();

    render_table_widget(
        frame,
        hit,
        &mut view.search_table,
        area,
        header,
        rows,
        &TABLE_WIDTHS,
    );
}
