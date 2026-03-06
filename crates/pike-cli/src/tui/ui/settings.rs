use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Row};
use rust_i18n::t;

use crate::tui::app::App;
use crate::tui::types::{HitState, SettingsRow, ViewState};

use super::{FG, FG_FAINT, GREEN, HOVER_FG, RED, render_table_widget};

pub(super) fn render_settings(
    frame: &mut Frame,
    app: &App,
    view: &mut ViewState,
    hit: &mut HitState,
    area: Rect,
) {
    let hover = view.hover_row;
    let layout = app.settings_layout();

    let rows: Vec<Row> = layout
        .iter()
        .enumerate()
        .map(|(idx, row)| match row {
            SettingsRow::Separator => Row::default().bottom_margin(1),
            SettingsRow::GroupHeader(label) => group_header_row(label),
            SettingsRow::LanguageCycle => {
                let label = t!("tui.settings.language");
                let display = match app.config.display.language.as_str() {
                    "en" => "English",
                    "pl" => "Polski",
                    _ => "auto",
                };
                value_row(&label, display.to_string(), hover == Some(idx))
            }
            SettingsRow::SourceToggle(st) => toggle_row(
                st.display_name(),
                app.config.sources.enabled(*st),
                hover == Some(idx),
            ),
            SettingsRow::SourcesReset => reset_row(hover == Some(idx)),
            SettingsRow::ArchToggle(st, arch) => {
                let enabled = app.config.display.architectures.arch_allowed(arch, *st);
                toggle_row(arch, enabled, hover == Some(idx))
            }
            SettingsRow::ArchReset(_) => reset_row(hover == Some(idx)),
            SettingsRow::LogToggle => {
                let label = t!("tui.settings.log-to-file");
                toggle_row(&label, app.config.logging.file, hover == Some(idx))
            }
            SettingsRow::DaemonStatus => {
                status_row(&t!("tui.settings.daemon-status"), app.daemon_running)
            }
            SettingsRow::DaemonInterval => {
                let label = t!("tui.settings.check-interval");
                value_row(
                    &label,
                    format_interval(app.config.daemon.interval),
                    hover == Some(idx),
                )
            }
            SettingsRow::NotifyToggle => {
                let label = t!("tui.settings.notifications");
                toggle_row(&label, app.config.daemon.notify, hover == Some(idx))
            }
        })
        .collect();

    let header = Row::default().bottom_margin(1);
    let widths = [Constraint::Length(25), Constraint::Min(10)];
    render_table_widget(
        frame,
        hit,
        &mut view.settings_table,
        area,
        header,
        rows,
        &widths,
    );
}

fn group_header_row(label: &str) -> Row<'static> {
    Row::new(vec![
        Cell::from(label.to_string())
            .style(Style::default().fg(FG_FAINT).add_modifier(Modifier::BOLD)),
        Cell::from(""),
    ])
}

fn reset_row(hovered: bool) -> Row<'static> {
    let style = if hovered {
        Style::default().fg(HOVER_FG)
    } else {
        Style::default().fg(FG_FAINT)
    };
    let label = format!("  {}", t!("tui.settings.reset"));
    Row::new(vec![Cell::from(label).style(style), Cell::from("")])
}

fn format_interval(secs: u64) -> String {
    if secs.is_multiple_of(3600) {
        format!("{}h", secs / 3600)
    } else if secs.is_multiple_of(60) {
        format!("{}min", secs / 60)
    } else {
        format!("{secs}s")
    }
}

fn value_row(key: &str, value: String, hovered: bool) -> Row<'static> {
    let name_style = if hovered {
        Style::default().fg(HOVER_FG)
    } else {
        Style::default().fg(FG)
    };
    let val_style = if hovered {
        Style::default().fg(HOVER_FG)
    } else {
        Style::default().fg(FG_FAINT)
    };
    Row::new(vec![
        Cell::from(format!("  {key}")).style(name_style),
        Cell::from(value).style(val_style),
    ])
}

fn status_row(key: &str, running: bool) -> Row<'static> {
    let (label, color) = if running {
        (t!("tui.settings.running").to_string(), GREEN)
    } else {
        (t!("tui.settings.stopped").to_string(), RED)
    };
    Row::new(vec![
        Cell::from(format!("  {key}")).style(Style::default().fg(FG_FAINT)),
        Cell::from(label).style(Style::default().fg(color)),
    ])
}

fn toggle_row(key: &str, value: bool, hovered: bool) -> Row<'static> {
    let name_style = if hovered {
        Style::default().fg(HOVER_FG)
    } else {
        Style::default().fg(FG)
    };
    let toggle = if value {
        Span::styled("●", Style::default().fg(GREEN))
    } else {
        Span::styled("○", Style::default().fg(RED))
    };
    Row::new(vec![
        Cell::from(format!("  {key}")).style(name_style),
        Cell::from(Line::from(toggle)),
    ])
}
