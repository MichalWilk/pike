use crossterm::event::KeyCode;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use rust_i18n::t;
use unicode_width::UnicodeWidthStr;

use crate::tui::app::App;
use crate::tui::types::{ClickAction, ClickTarget, HitState, InputMode, Tab, ViewState};

use super::{ACCENT, FG, FG_DIM, FG_FAINT, FG_SUBTLE, RED, format_count_line};

struct ButtonDef {
    key: &'static str,
    label: String,
    code: KeyCode,
}

impl ButtonDef {
    fn new(key: &'static str, label_key: &str, code: KeyCode) -> Self {
        Self {
            key,
            label: t!(label_key).to_string(),
            code,
        }
    }
}

pub(super) fn render_title(frame: &mut Frame, area: Rect) {
    let title = Line::from(Span::styled(
        "pike",
        Style::default().fg(FG).add_modifier(Modifier::BOLD),
    ))
    .centered();
    frame.render_widget(Paragraph::new(title), area);
}

const LEFT_TABS: [Tab; 4] = [Tab::Search, Tab::Installed, Tab::Updates, Tab::Repos];
const RIGHT_TABS: [Tab; 2] = [Tab::Settings, Tab::About];

fn tab_label(tab: Tab, app: &App) -> (String, String) {
    let num = tab.key().to_string();
    let name = if tab == Tab::Updates && !app.updates.items.is_empty() {
        format!(" {} ({})", tab.title(), app.updates.items.len())
    } else {
        format!(" {}", tab.title())
    };
    (num, name)
}

fn tab_group_width(tabs: &[Tab], app: &App) -> usize {
    tabs.iter()
        .map(|&t| {
            let (n, l) = tab_label(t, app);
            n.width() + l.width()
        })
        .sum::<usize>()
        + tabs.len().saturating_sub(1) * 3
}

fn render_tab_group(
    tabs: &[Tab],
    app: &App,
    spans: &mut Vec<Span<'static>>,
    hit: &mut HitState,
    x: &mut u16,
    y: u16,
) {
    for (i, &tab) in tabs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("   ", Style::default().fg(FG_SUBTLE)));
            *x += 3;
        }
        let start_x = *x;
        let (num, name) = tab_label(tab, app);
        let is_active = tab == app.tab;

        spans.push(Span::styled(num, Style::default().fg(ACCENT)));
        *x += tab.key().width() as u16;

        let name_style = if is_active {
            Style::default().fg(FG).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(FG_DIM)
        };
        let name_w = name.width() as u16;
        spans.push(Span::styled(name, name_style));
        *x += name_w;

        hit.click_targets.push(ClickTarget {
            rect: Rect::new(start_x, y, *x - start_x, 1),
            action: ClickAction::SwitchTab(tab),
        });
    }
}

pub(super) fn render_tab_bar(frame: &mut Frame, app: &App, hit: &mut HitState, area: Rect) {
    let left_w = tab_group_width(&LEFT_TABS, app);
    let right_w = tab_group_width(&RIGHT_TABS, app);
    let gap = (area.width as usize).saturating_sub(left_w + right_w);

    let mut spans: Vec<Span> = Vec::new();
    let mut x = area.x;

    render_tab_group(&LEFT_TABS, app, &mut spans, hit, &mut x, area.y);

    if gap > 0 {
        spans.push(Span::raw(" ".repeat(gap)));
        x += gap as u16;
    }

    render_tab_group(&RIGHT_TABS, app, &mut spans, hit, &mut x, area.y);

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

pub(super) fn render_context_bar(frame: &mut Frame, app: &App, area: Rect) {
    let missing = app.missing_backends();
    if !missing.is_empty() {
        let warning = t!("tui.context.not-found", names = missing.join(", "));
        let line = Line::from(vec![Span::styled(
            warning.to_string(),
            Style::default().fg(RED),
        )]);
        frame.render_widget(Paragraph::new(line), area);
        return;
    }

    let line = match app.tab {
        Tab::Search => {
            if app.search.results.loading || app.search.results.items.is_empty() {
                return;
            }
            let shown = app.search_filtered_indices().len();
            format_count_line(
                &app.search.results.items,
                shown,
                |p| p.source,
                "tui.context.results",
            )
        }
        Tab::Installed => {
            if app.installed.loading || app.installed.items.is_empty() {
                return;
            }
            let shown = app.installed_filtered_indices().len();
            format_count_line(
                &app.installed.items,
                shown,
                |p| p.source,
                "tui.context.packages",
            )
        }
        Tab::Updates => {
            if app.updates.loading || app.updates.items.is_empty() {
                return;
            }
            let shown = app.updates_filtered_indices().len();
            format_count_line(
                &app.updates.items,
                shown,
                |u| u.source,
                "tui.context.updates-available",
            )
        }
        Tab::Repos => match repos_context_line(app) {
            Some(line) => line,
            None => return,
        },
        Tab::Settings | Tab::About => return,
    };

    frame.render_widget(Paragraph::new(line), area);
}

pub(super) fn render_footer(
    frame: &mut Frame,
    app: &App,
    view: &ViewState,
    hit: &mut HitState,
    area: Rect,
) {
    let (action_btns, quit_btn) = footer_buttons(app, view);

    let mut left_spans: Vec<Span> = Vec::new();
    let mut x = area.x;

    for (i, btn) in action_btns.iter().enumerate() {
        if i > 0 {
            left_spans.push(Span::raw("     "));
            x += 5;
        }

        let start_x = x;
        let key_len = btn.key.width() as u16;
        left_spans.push(Span::styled(btn.key, Style::default().fg(ACCENT)));
        x += key_len;

        let label = format!(" {}", btn.label);
        let label_len = label.width() as u16;
        left_spans.push(Span::styled(label, Style::default().fg(FG_DIM)));
        x += label_len;

        hit.click_targets.push(ClickTarget {
            rect: Rect::new(start_x, area.y, x - start_x, 1),
            action: ClickAction::Key(btn.code),
        });
    }

    if !app.status_message.is_empty() {
        left_spans.push(Span::raw("     "));
        left_spans.push(Span::styled(
            &app.status_message,
            Style::default().fg(FG_DIM),
        ));
    }

    let quit_text = format!("{} {}", quit_btn.key, quit_btn.label);
    let quit_width = quit_text.width() as u16;
    let quit_x = area.x + area.width.saturating_sub(quit_width);

    let right_line = Line::from(vec![
        Span::styled(quit_btn.key, Style::default().fg(ACCENT)),
        Span::styled(format!(" {}", quit_btn.label), Style::default().fg(FG_DIM)),
    ]);

    hit.click_targets.push(ClickTarget {
        rect: Rect::new(quit_x, area.y, quit_width, 1),
        action: ClickAction::Key(quit_btn.code),
    });

    frame.render_widget(Paragraph::new(Line::from(left_spans)), area);

    let right_area = Rect::new(quit_x, area.y, quit_width, 1);
    frame.render_widget(Paragraph::new(right_line), right_area);
}

fn repos_context_line(app: &App) -> Option<Line<'static>> {
    if app.repos.add_form.active {
        return None;
    }
    if app.repos.list.loading || app.repos.list.items.is_empty() {
        return None;
    }
    let total = app.repos.list.items.len();
    let filtered = app.repos_filtered_indices();
    let shown = filtered.len();
    let enabled = filtered
        .iter()
        .filter(|&&i| app.repos.list.items[i].enabled)
        .count();
    let text = if shown < total {
        t!(
            "tui.context.repos-filtered",
            shown = shown,
            total = total,
            enabled = enabled
        )
        .to_string()
    } else {
        t!(
            &crate::i18n::plural_key("tui.context.repos-total", total),
            total = total,
            enabled = enabled
        )
        .to_string()
    };
    Some(Line::from(Span::styled(
        text,
        Style::default().fg(FG_FAINT),
    )))
}

fn footer_buttons(app: &App, view: &ViewState) -> (Vec<ButtonDef>, ButtonDef) {
    let quit = ButtonDef::new("q", "tui.button.quit", KeyCode::Char('q'));
    let actions = match app.tab {
        Tab::Search => search_buttons(app, view),
        Tab::Installed => installed_buttons(app),
        Tab::Updates => updates_buttons(app),
        Tab::Repos => repos_buttons(app),
        Tab::Settings => settings_buttons(),
        Tab::About => vec![ButtonDef::new("↵", "tui.about.open-link", KeyCode::Enter)],
    };
    (actions, quit)
}

fn source_filter_button(app: &App) -> ButtonDef {
    let label = match app.current_source_filter() {
        None => t!("tui.button.source-all").to_string(),
        Some(st) => t!("tui.button.source-named", source = st.display_name()).to_string(),
    };
    ButtonDef {
        key: "s",
        label,
        code: KeyCode::Char('s'),
    }
}

fn editing_buttons(search_tab: bool) -> Vec<ButtonDef> {
    vec![
        ButtonDef::new("Esc", "tui.button.clear", KeyCode::Esc),
        ButtonDef {
            key: "↵",
            label: if search_tab {
                t!("tui.button.search")
            } else {
                t!("tui.button.done")
            }
            .to_string(),
            code: KeyCode::Enter,
        },
    ]
}

fn search_buttons(app: &App, view: &ViewState) -> Vec<ButtonDef> {
    if app.input_mode == InputMode::Editing {
        return editing_buttons(true);
    }
    let mut btns = vec![
        ButtonDef::new("/", "tui.button.search", KeyCode::Char('/')),
        source_filter_button(app),
    ];
    if !app.search.input.is_empty() {
        btns.push(ButtonDef::new(
            "r",
            "tui.button.refresh",
            KeyCode::Char('r'),
        ));
    }
    let selected_installed = app
        .selected_search_package(view)
        .map(|pkg| app.is_installed(&pkg.name, pkg.source));
    match selected_installed {
        Some(true) => btns.push(ButtonDef::new("d", "tui.button.remove", KeyCode::Char('d'))),
        Some(false) => btns.push(ButtonDef::new(
            "i",
            "tui.button.install",
            KeyCode::Char('i'),
        )),
        None => {
            btns.push(ButtonDef::new(
                "i",
                "tui.button.install",
                KeyCode::Char('i'),
            ));
            btns.push(ButtonDef::new("d", "tui.button.remove", KeyCode::Char('d')));
        }
    }
    btns
}

fn installed_buttons(app: &App) -> Vec<ButtonDef> {
    if app.input_mode == InputMode::Editing {
        return editing_buttons(false);
    }
    vec![
        ButtonDef::new("/", "tui.button.filter", KeyCode::Char('/')),
        source_filter_button(app),
        ButtonDef::new("d", "tui.button.remove", KeyCode::Char('d')),
        ButtonDef::new("A", "tui.button.autoremove", KeyCode::Char('A')),
        ButtonDef::new("r", "tui.button.refresh", KeyCode::Char('r')),
    ]
}

fn updates_buttons(app: &App) -> Vec<ButtonDef> {
    if app.input_mode == InputMode::Editing {
        return editing_buttons(false);
    }
    let mut btns = vec![
        ButtonDef::new("/", "tui.button.filter", KeyCode::Char('/')),
        source_filter_button(app),
    ];
    if !app.updates.items.is_empty() {
        btns.push(ButtonDef::new("u", "tui.button.update", KeyCode::Char('u')));
        btns.push(ButtonDef::new("U", "tui.button.all", KeyCode::Char('U')));
    }
    btns.push(ButtonDef::new(
        "r",
        "tui.button.refresh",
        KeyCode::Char('r'),
    ));
    btns
}

fn repos_buttons(app: &App) -> Vec<ButtonDef> {
    if app.repos.add_form.active {
        return repos_form_buttons(app);
    }
    if app.input_mode == InputMode::Editing {
        return editing_buttons(false);
    }
    vec![
        ButtonDef::new("/", "tui.button.filter", KeyCode::Char('/')),
        source_filter_button(app),
        ButtonDef::new("e", "tui.button.toggle", KeyCode::Char('e')),
        ButtonDef::new("a", "tui.button.add", KeyCode::Char('a')),
        ButtonDef::new("d", "tui.button.delete", KeyCode::Char('d')),
        ButtonDef::new("r", "tui.button.refresh", KeyCode::Char('r')),
    ]
}

fn repos_form_buttons(app: &App) -> Vec<ButtonDef> {
    let mut btns = vec![ButtonDef::new("Esc", "tui.button.cancel", KeyCode::Esc)];

    if app.repos.add_form.step < 2 {
        btns.push(ButtonDef::new("j/k", "tui.button.select", KeyCode::Up));
        btns.push(ButtonDef::new(
            "e",
            "tui.button.confirm",
            KeyCode::Char('e'),
        ));
    } else {
        let fields = app.repos.add_form.fields();
        if fields.len() > 1 {
            btns.push(ButtonDef::new(
                "Tab/S-Tab",
                "tui.button.fields",
                KeyCode::Tab,
            ));
        }
        if app.repos.add_form.active_field() == Some(crate::tui::types::FormField::GpgCheck) {
            btns.push(ButtonDef::new(
                "e",
                "tui.button.toggle-gpg",
                KeyCode::Char('e'),
            ));
        }
        btns.push(ButtonDef::new("↵", "tui.button.confirm", KeyCode::Enter));
    }

    btns
}

fn settings_buttons() -> Vec<ButtonDef> {
    vec![ButtonDef::new("e", "tui.button.toggle", KeyCode::Char('e'))]
}
