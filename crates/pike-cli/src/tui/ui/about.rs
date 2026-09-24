use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use super::{FG, FG_FAINT, accent};
use crate::tui::types::{ClickAction, ClickTarget, HitState};

pub(crate) const ABOUT_URLS: [&str; 2] = [
    "https://github.com/MichalWilk/pike",
    "https://ko-fi.com/F1F11VG9MO",
];

const ABOUT_LABELS: [&str; 2] = ["github.com/MichalWilk/pike", "ko-fi.com/F1F11VG9MO"];

const FIRST_LINK_LINE: u16 = 6;

pub(super) fn render_about(frame: &mut Frame, hit: &mut HitState, area: Rect, selected: usize) {
    let art_style = Style::default().fg(accent());
    let fish_style = Style::default().fg(accent()).add_modifier(Modifier::BOLD);
    let name_style = Style::default().fg(FG).add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(FG_FAINT);

    let link_style = |i: usize| {
        let color = if i == selected { accent() } else { FG_FAINT };
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

    for (i, label) in ABOUT_LABELS.iter().enumerate() {
        let y = centered_v.y + FIRST_LINK_LINE + i as u16;
        if y >= centered_v.bottom() {
            break;
        }
        let width = (label.width() as u16).min(centered_v.width);
        let offset = (centered_v.width / 2).saturating_sub(width / 2);
        hit.click_targets.push(ClickTarget {
            rect: Rect::new(centered_v.x + offset, y, width, 1),
            action: ClickAction::AboutLink(i),
        });
    }

    frame.render_widget(Paragraph::new(lines), centered_v);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn test_link_targets_cover_rendered_labels() {
        let mut terminal = Terminal::new(TestBackend::new(61, 20)).unwrap();
        let mut hit = HitState::default();
        terminal
            .draw(|frame| render_about(frame, &mut hit, frame.area(), 0))
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(hit.click_targets.len(), ABOUT_LABELS.len());
        for (target, label) in hit.click_targets.iter().zip(ABOUT_LABELS) {
            let r = target.rect;
            let text: String = (r.x..r.x + r.width)
                .map(|x| buffer[(x, r.y)].symbol())
                .collect();
            assert_eq!(text, label);
        }
    }
}
