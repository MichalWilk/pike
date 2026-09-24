use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph};
use rust_i18n::t;
use unicode_width::UnicodeWidthChar;

use crate::format::split_clean_preview;
use crate::tui::types::{Action, PendingConfirm, ViewState};

use super::{ACCENT, FG, FG_FAINT, RED};

fn capped<'a>(lines: impl ExactSizeIterator<Item = Line<'a>>, max: usize) -> Vec<Line<'a>> {
    let total = lines.len();
    if total <= max {
        return lines.collect();
    }
    let keep = max.saturating_sub(1);
    let mut out: Vec<_> = lines.take(keep).collect();
    if max > 0 {
        let more = t!("tui.confirm.more", count = total - keep).to_string();
        out.push(Line::styled(more, Style::default().fg(FG_FAINT)));
    }
    out
}

#[derive(Debug, PartialEq, Eq)]
struct Budget {
    items: usize,
    preview: usize,
    spaced: bool,
}

fn line_budget(body: usize, items: usize, preview: usize) -> Budget {
    let spaced = preview > 0 && body >= 2 + preview.min(2);
    let room = body - usize::from(spaced);
    let share = items.min(room / 2).max(1);
    let preview = preview.min(room.saturating_sub(share).max(2));
    let items = room.saturating_sub(preview).max(1);
    Budget {
        items,
        preview,
        spaced,
    }
}

fn wrap_chars(text: &str, width: usize) -> Vec<String> {
    let mut rows = Vec::new();
    let mut row = String::new();
    let mut row_width = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if row_width + w > width && !row.is_empty() {
            rows.push(std::mem::take(&mut row));
            row_width = 0;
        }
        row.push(c);
        row_width += w;
    }
    rows.push(row);
    rows
}

fn preview_lines(
    pending: &PendingConfirm,
    view: &ViewState,
) -> (Option<Line<'static>>, Vec<Line<'static>>) {
    if !matches!(pending.action, Action::Clean(_)) {
        return (None, Vec::new());
    }
    let Some(preview) = &pending.preview else {
        let text = format!(
            "{} {}",
            view.spinner_char(),
            t!("tui.confirm.checking-deps")
        );
        return (
            None,
            vec![Line::styled(text, Style::default().fg(FG_FAINT))],
        );
    };
    let (extras, errors) = split_clean_preview(preview);
    let header = (!extras.is_empty()).then(|| {
        Line::styled(
            t!("tui.confirm.also-removed").to_string(),
            Style::default().fg(ACCENT),
        )
    });
    let extras = extras
        .into_iter()
        .map(|(st, p)| Line::styled(format!("  [{st}] {p}"), Style::default().fg(FG)));
    let errors = errors.into_iter().map(|(st, e)| {
        Line::styled(
            format!("  [{st}] {}", t!("tui.confirm.preview-failed", err = e)),
            Style::default().fg(RED),
        )
    });
    (header, extras.chain(errors).collect())
}

pub(super) fn render_confirm(
    frame: &mut Frame,
    pending: &PendingConfirm,
    view: &ViewState,
) -> Rect {
    let screen = frame.area();
    let width = (screen.width * 6 / 10).clamp(40, 90).min(screen.width);
    let max_height = screen.height.saturating_sub(4).max(6).min(screen.height);
    let max_body = max_height.saturating_sub(2) as usize;
    let inner_width = width.saturating_sub(4) as usize;

    let (header, preview) = preview_lines(pending, view);
    let preview_len = usize::from(header.is_some()) + preview.len();

    let item_rows: Vec<String> = if matches!(pending.action, Action::Clean(_)) {
        pending.lines.clone()
    } else {
        pending
            .lines
            .iter()
            .flat_map(|l| wrap_chars(l, inner_width))
            .collect()
    };
    let budget = line_budget(max_body, item_rows.len(), preview_len);
    let items = item_rows
        .into_iter()
        .map(|l| Line::styled(l, Style::default().fg(FG)));
    let mut body = if matches!(pending.action, Action::Clean(_)) {
        capped(items, budget.items)
    } else {
        items.take(budget.items).collect()
    };
    if budget.preview > 0 {
        if budget.spaced {
            body.push(Line::default());
        }
        let mut room = budget.preview;
        if let Some(header) = header {
            body.push(header);
            room -= 1;
        }
        body.extend(capped(preview.into_iter(), room));
    }

    let height = (body.len() as u16 + 2).min(max_height);
    let area = Rect::new(
        screen.x + (screen.width - width) / 2,
        screen.y + (screen.height - height) / 2,
        width,
        height,
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(ACCENT))
        .padding(Padding::horizontal(1))
        .title(Span::styled(
            format!(" {} ", pending.title),
            Style::default().fg(FG).add_modifier(Modifier::BOLD),
        ));
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(body).block(block), area);
    area
}

#[cfg(test)]
mod tests {
    use super::*;
    use pike_core::package::{RepoMethod, SourceType};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::tui::types::AddRepoParams;

    #[test]
    fn test_line_budget_shows_all_deps_of_one_item() {
        let budget = line_budget(18, 1, 13);
        assert_eq!(
            budget,
            Budget {
                items: 4,
                preview: 13,
                spaced: true
            }
        );
    }

    #[test]
    fn test_line_budget_splits_long_lists() {
        let budget = line_budget(18, 40, 40);
        assert_eq!((budget.items, budget.preview), (8, 9));
        assert_eq!(line_budget(18, 40, 0).items, 18);
        assert_eq!(line_budget(18, 40, 1).items, 16);
    }

    #[test]
    fn test_line_budget_tight_keeps_header_with_an_entry() {
        let budget = line_budget(4, 5, 13);
        assert_eq!(
            budget,
            Budget {
                items: 1,
                preview: 2,
                spaced: true
            }
        );
        for body in 1..=6 {
            let budget = line_budget(body, 5, 13);
            assert!(budget.preview >= 2, "body {body}: {budget:?}");
            if body >= 3 {
                let used = budget.items + budget.preview + usize::from(budget.spaced);
                assert!(used <= body, "body {body}: {budget:?}");
            }
        }
    }

    #[test]
    fn test_add_repo_modal_shows_full_url() {
        let url = "https://download.docker.com/linux/fedora/docker-ce.repo";
        let pending = PendingConfirm {
            action: Action::AddRepo(AddRepoParams {
                method: RepoMethod::RepoFile,
                repo_id: String::new(),
                name: String::new(),
                url: url.into(),
                source: SourceType::Dnf,
                gpgcheck: true,
            }),
            title: "Add repository?".into(),
            lines: vec![format!("URL: {url}"), "Source: dnf".into()],
            preview: None,
        };
        let view = ViewState::new(false);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut area = Rect::default();
        terminal
            .draw(|frame| area = render_confirm(frame, &pending, &view))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let text: String = (area.top() + 1..area.bottom() - 1)
            .flat_map(|y| (area.left() + 2..area.right() - 2).map(move |x| (x, y)))
            .map(|(x, y)| buffer[(x, y)].symbol().to_string())
            .collect();
        assert!(text.contains(url), "{text}");
    }

    fn rendered_text(pending: &PendingConfirm) -> String {
        let view = ViewState::new(false);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut area = Rect::default();
        terminal
            .draw(|frame| area = render_confirm(frame, pending, &view))
            .unwrap();
        let buffer = terminal.backend().buffer();
        (area.top() + 1..area.bottom() - 1)
            .flat_map(|y| (area.left() + 2..area.right() - 2).map(move |x| (x, y)))
            .map(|(x, y)| buffer[(x, y)].symbol().to_string())
            .collect()
    }

    #[test]
    fn test_non_clean_dialog_truncates_without_more_line() {
        let lines: Vec<String> = (0..30).map(|i| format!("line {i}")).collect();
        let pending = PendingConfirm {
            action: Action::RemovePackage("pkg".into(), None),
            title: "Remove pkg?".into(),
            lines,
            preview: None,
        };
        let text = rendered_text(&pending);
        assert!(!text.contains("more"), "{text}");
    }

    #[test]
    fn test_clean_dialog_keeps_more_line() {
        use pike_core::package::{CleanupItem, CleanupKind};

        let items: Vec<CleanupItem> = (0..30)
            .map(|i| CleanupItem {
                source: SourceType::Dnf,
                kind: CleanupKind::Orphan,
                name: format!("pkg{i}"),
                version: "1".into(),
                size: Some(1024),
                arch: None,
            })
            .collect();
        let lines: Vec<String> = items
            .iter()
            .map(|item| format!("[{}] {} {}", item.source, item.name, item.version))
            .collect();
        let pending = PendingConfirm {
            action: Action::Clean(items),
            title: "Clean 30 items?".into(),
            lines,
            preview: None,
        };
        let text = rendered_text(&pending);
        assert!(text.contains("more"), "{text}");
    }
}
