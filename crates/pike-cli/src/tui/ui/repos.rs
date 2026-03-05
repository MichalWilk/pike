use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row};
use rust_i18n::t;

use crate::tui::types::FormField;
use pike_core::package::SourceType;
use pike_core::util::truncate_str;

use crate::tui::app::App;
use crate::tui::types::{HitState, Tab, ViewState};

use super::{
    ACCENT, FG, FG_DIM, FG_FAINT, GREEN, RED, render_centered_empty, render_table_widget,
    row_styles, split_filter_area,
};

pub(super) fn render_repos(
    frame: &mut Frame,
    app: &App,
    view: &mut ViewState,
    hit: &mut HitState,
    area: Rect,
) {
    if app.repos.add_form.active {
        render_repos_add_form(frame, app, area);
        return;
    }

    if app.repos.list.loading {
        let msg = t!("tui.repos.loading");
        render_centered_empty(frame, area, &view.spinner_char().to_string(), &msg, ACCENT);
        return;
    }

    if app.repos.list.items.is_empty() {
        let msg = t!("tui.repos.empty");
        render_centered_empty(frame, area, "-", &msg, FG_FAINT);
        return;
    }

    let editing = app.is_editing_on(Tab::Repos);
    let table_area = split_filter_area(frame, &app.repos.list.filter, editing, area);

    let filtered = app.repos_filtered_indices();

    if filtered.is_empty() {
        let msg = t!("tui.repos.no-match");
        render_centered_empty(frame, table_area, "∅", &msg, FG_FAINT);
        return;
    }

    render_repos_table(frame, app, view, hit, table_area, &filtered);
}

fn render_repos_table(
    frame: &mut Frame,
    app: &App,
    view: &mut ViewState,
    hit: &mut HitState,
    area: Rect,
    filtered: &[usize],
) {
    let hover = view.hover_row;

    let header = Row::new([
        t!("header.source").to_string(),
        t!("header.repo-id").to_string(),
        t!("header.name").to_string(),
        t!("header.status").to_string(),
    ])
    .style(Style::default().fg(FG_FAINT))
    .bottom_margin(1);

    let rows: Vec<Row> = filtered
        .iter()
        .enumerate()
        .map(|(vi, &real_i)| {
            let repo = &app.repos.list.items[real_i];
            let hov = hover == Some(vi);
            let status_c = if repo.enabled { GREEN } else { RED };
            let (src_style, id_style, name_style, status_style) = row_styles(hov, status_c);
            let status_char = if repo.enabled { "●" } else { "○" };
            Row::new(vec![
                Cell::from(repo.source.to_string()).style(src_style),
                Cell::from(repo.id.as_str()).style(id_style),
                Cell::from(truncate_str(&repo.name, 30).into_owned()).style(name_style),
                Cell::from(status_char).style(status_style),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(10),
        Constraint::Length(25),
        Constraint::Length(32),
        Constraint::Length(6),
    ];

    render_table_widget(
        frame,
        hit,
        &mut view.repos_table,
        area,
        header,
        rows,
        &widths,
    );
}

fn render_repos_add_form(frame: &mut Frame, app: &App, area: Rect) {
    match app.repos.add_form.step {
        0 => render_repos_picker(frame, app, area),
        _ => render_repos_fields_form(frame, app, area),
    }
}

fn render_form_header(frame: &mut Frame, app: &App, area: Rect) {
    let header_area = Rect::new(area.x, area.y, area.width, 1);
    let sep = Span::styled(" ‹ ", Style::default().fg(FG_FAINT));
    let title = t!("tui.repos.form-title");
    let mut spans = vec![
        Span::styled("esc", Style::default().fg(ACCENT)),
        sep.clone(),
        Span::styled(title.to_string(), Style::default().fg(FG_FAINT)),
    ];
    if let Some(source) = app.repos.add_form.source
        && app.repos.add_form.step >= 2
    {
        spans.push(sep.clone());
        spans.push(Span::styled(
            source.display_name(),
            Style::default().fg(FG_FAINT),
        ));
        if let Some(method) = app.repos.add_form.method {
            spans.push(sep);
            spans.push(Span::styled(
                method.display_name(),
                Style::default().fg(FG_DIM),
            ));
        }
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), header_area);
}

fn render_repos_picker(frame: &mut Frame, app: &App, area: Rect) {
    render_form_header(frame, app, area);

    let entries = app.picker_entries();
    let selected_idx = app.repos.add_form.method_index;
    let mut y = area.y + 2;
    let mut prev_source: Option<SourceType> = None;

    for (i, &(source, method)) in entries.iter().enumerate() {
        if prev_source != Some(source) {
            if prev_source.is_some() {
                y += 1;
            }
            let header_area = Rect::new(area.x, y, area.width, 1);
            frame.render_widget(
                Paragraph::new(Span::styled(
                    source.display_name(),
                    Style::default().fg(FG_FAINT),
                )),
                header_area,
            );
            y += 1;
            prev_source = Some(source);
        }

        let row_area = Rect::new(area.x, y, area.width, 1);
        let is_selected = i == selected_idx;
        let style = if is_selected {
            Style::default()
                .bg(super::SELECTED_BG)
                .fg(super::SELECTED_FG)
        } else {
            Style::default().fg(FG_DIM)
        };
        frame.render_widget(
            Paragraph::new(Span::styled(format!("  {}", method.display_name()), style)),
            row_area,
        );
        y += 1;
    }
}

fn render_form_field(
    frame: &mut Frame,
    area: Rect,
    y: u16,
    label: &str,
    value: &str,
    active: bool,
    cursor: bool,
) {
    let field_area = Rect::new(area.x, y, area.width, 1);
    let style = if active {
        Style::default().fg(ACCENT)
    } else {
        Style::default().fg(FG_FAINT)
    };
    let line = Line::from(vec![
        Span::styled(label.to_string(), style),
        Span::styled(value, Style::default().fg(FG)),
    ]);
    frame.render_widget(Paragraph::new(line), field_area);
    if cursor {
        use unicode_width::UnicodeWidthStr;
        frame.set_cursor_position((
            field_area.x + label.width() as u16 + value.width() as u16,
            y,
        ));
    }
}

fn render_gpgcheck_toggle(frame: &mut Frame, area: Rect, y: u16, active: bool, checked: bool) {
    let field_area = Rect::new(area.x, y, area.width, 1);
    let label_style = if active {
        Style::default().fg(ACCENT)
    } else {
        Style::default().fg(FG_FAINT)
    };
    let indicator = if checked { "●" } else { "○" };
    let value_color = if checked { GREEN } else { RED };
    let label = t!("tui.repos.label-gpgcheck");
    let line = Line::from(vec![
        Span::styled(label.to_string(), label_style),
        Span::styled(indicator, Style::default().fg(value_color)),
    ]);
    frame.render_widget(Paragraph::new(line), field_area);
}

fn render_repos_fields_form(frame: &mut Frame, app: &App, area: Rect) {
    render_form_header(frame, app, area);

    let fields = app.repos.add_form.fields();
    let method = app
        .repos
        .add_form
        .method
        .unwrap_or(pike_core::package::RepoMethod::RemoteAdd);
    let active_idx = app.repos.add_form.field as usize;

    let labels: Vec<String> = fields
        .iter()
        .map(|f| match f {
            FormField::RepoId => t!("tui.repos.label-repo-id").to_string(),
            FormField::Name => {
                if method.needs_name() {
                    t!("tui.repos.label-name").to_string()
                } else {
                    t!("tui.repos.label-name-optional").to_string()
                }
            }
            FormField::Url => format!("{}: ", method.url_label()),
            FormField::GpgCheck => t!("tui.repos.label-gpgcheck").to_string(),
        })
        .collect();

    let max_label_len = labels.iter().map(|l| l.len()).max().unwrap_or(0);

    let mut y = area.y + 2;
    for (i, (field, label)) in fields.iter().zip(labels.iter()).enumerate() {
        let is_active = i == active_idx;
        let padded = format!("{:<width$}", label, width = max_label_len);
        match field {
            FormField::GpgCheck => {
                render_gpgcheck_toggle(frame, area, y, is_active, app.repos.add_form.gpgcheck);
            }
            _ => {
                if let Some(value) = app.repos.add_form.field_text(*field) {
                    render_form_field(frame, area, y, &padded, value, is_active, is_active);
                }
            }
        }
        y += 1;
        if matches!(field, FormField::GpgCheck) {
            y += 1;
        }
    }

    if !app.repos.add_form.validation_error.is_empty() {
        y += 1;
        let err_area = Rect::new(area.x, y, area.width, 1);
        frame.render_widget(
            Paragraph::new(Span::styled(
                &app.repos.add_form.validation_error,
                Style::default().fg(RED),
            )),
            err_area,
        );
    }
}
