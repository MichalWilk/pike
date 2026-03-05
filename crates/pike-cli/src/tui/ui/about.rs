use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{ACCENT, FG, FG_FAINT};

pub(crate) const ABOUT_URLS: [&str; 2] = [
    "https://github.com/MichalWilk/pike",
    "https://ko-fi.com/F1F11VG9MO",
];

const ABOUT_LABELS: [&str; 2] = ["github.com/MichalWilk/pike", "ko-fi.com/F1F11VG9MO"];

pub(super) fn render_about(frame: &mut Frame, area: Rect, selected: usize) {
    let art_style = Style::default().fg(ACCENT);
    let fish_style = Style::default().fg(ACCENT).add_modifier(Modifier::BOLD);
    let name_style = Style::default().fg(FG).add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(FG_FAINT);

    let link_style = |i: usize| {
        let color = if i == selected { ACCENT } else { FG_FAINT };
        Style::default()
            .fg(color)
            .add_modifier(Modifier::UNDERLINED)
    };

    let lines: Vec<Line> = vec![
        Line::from(Span::styled(r"   \o   ╶──╮", art_style)).centered(),
        Line::from(Span::styled(r"    |\     │", art_style)).centered(),
        Line::from(vec![
            Span::styled(r"    /\  ", art_style),
            Span::styled("><(((°>", fish_style),
        ])
        .centered(),
        Line::from(""),
        Line::from(vec![
            Span::styled("PIKE", name_style),
            Span::styled(format!("  v{}", env!("CARGO_PKG_VERSION")), dim),
        ])
        .centered(),
        Line::from(Span::styled("Unified Package Manager", dim)).centered(),
        Line::from(Span::styled(ABOUT_LABELS[0], link_style(0))).centered(),
        Line::from(Span::styled(ABOUT_LABELS[1], link_style(1))).centered(),
        Line::from(""),
        Line::from(Span::styled("MIT License", dim)).centered(),
    ];

    let content_height = lines.len() as u16;

    let [centered_v] = Layout::vertical([Constraint::Length(content_height)])
        .flex(Flex::Center)
        .areas(area);

    frame.render_widget(Paragraph::new(lines), centered_v);
}
