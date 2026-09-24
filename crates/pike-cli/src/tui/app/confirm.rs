use crossterm::event::{KeyCode, KeyEvent};
use pike_core::cleanup::total_size;
use pike_core::package::{CleanupItem, SourceType};
use rust_i18n::t;

use super::{App, ReposAddForm, name_from_url};
use crate::format::{cleanup_version, format_size};
use crate::i18n::plural_key;
use crate::tui::types::{Action, AddRepoParams, CleanPreview, InputMode, PendingConfirm};

fn source_lines(source: Option<SourceType>) -> Vec<String> {
    source
        .map(|st| t!("tui.confirm.source", source = st.display_name()).to_string())
        .into_iter()
        .collect()
}

fn clean_line(item: &CleanupItem) -> String {
    [
        format!("[{}]", item.source),
        item.name.clone(),
        cleanup_version(item),
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(" ")
}

fn confirm_text(action: &Action) -> Option<(String, Vec<String>)> {
    let text = match action {
        Action::InstallPackage(pkg, source) => (
            t!("tui.confirm.install", pkg = pkg).to_string(),
            source_lines(*source),
        ),
        Action::RemovePackage(pkg, source) => (
            t!("tui.confirm.remove", pkg = pkg).to_string(),
            source_lines(*source),
        ),
        Action::ToggleRepo(id, enabled, source) => {
            let title = if *enabled {
                t!("tui.confirm.enable-repo")
            } else {
                t!("tui.confirm.disable-repo")
            };
            let mut lines = vec![t!("tui.confirm.repo-id", id = id).to_string()];
            lines.extend(source_lines(Some(*source)));
            (title.to_string(), lines)
        }
        Action::DeleteRepo(id, source) => {
            let mut lines = vec![t!("tui.confirm.repo-id", id = id).to_string()];
            lines.extend(source_lines(Some(*source)));
            (t!("tui.confirm.delete-repo").to_string(), lines)
        }
        Action::AddRepo(params) => {
            let title = if params.name.is_empty() {
                t!("tui.confirm.add-repo-unnamed")
            } else {
                t!("tui.confirm.add-repo", name = &params.name)
            };
            let mut lines = vec![t!("tui.confirm.url", url = &params.url).to_string()];
            if !params.repo_id.is_empty() {
                lines.push(t!("tui.confirm.repo-id", id = &params.repo_id).to_string());
            }
            lines.extend(source_lines(Some(params.source)));
            (title.to_string(), lines)
        }
        Action::Clean(items) => (
            t!(
                &plural_key("tui.confirm.clean", items.len()),
                count = items.len(),
                size = format_size(total_size(items))
            )
            .to_string(),
            items.iter().map(clean_line).collect(),
        ),
        _ => return None,
    };
    Some(text)
}

impl App {
    pub(super) fn intercept(&mut self, action: Action) -> Option<Action> {
        if !self.config.display.confirm_actions {
            return Some(action);
        }
        let Some((title, lines)) = confirm_text(&action) else {
            return Some(action);
        };
        self.pending_confirm = Some(PendingConfirm {
            action,
            title,
            lines,
            preview: None,
        });
        None
    }

    pub(super) fn intercept_all(&mut self, actions: Vec<Action>) -> Vec<Action> {
        actions
            .into_iter()
            .filter_map(|action| self.intercept(action))
            .collect()
    }

    pub(crate) fn pending_clean_items(&self) -> Option<&[CleanupItem]> {
        match self.pending_confirm.as_ref().map(|p| &p.action) {
            Some(Action::Clean(items)) => Some(items),
            _ => None,
        }
    }

    pub(crate) fn set_clean_preview(&mut self, items: &[CleanupItem], preview: CleanPreview) {
        if self.pending_clean_items() == Some(items)
            && let Some(pending) = &mut self.pending_confirm
        {
            pending.preview = Some(preview);
        }
    }

    fn reopen_add_form(&mut self, params: AddRepoParams) {
        let method_index = self
            .picker_entries()
            .iter()
            .position(|&(st, m)| st == params.source && m == params.method)
            .unwrap_or(0);
        let name = if params.name == name_from_url(&params.url) {
            String::new()
        } else {
            params.name
        };
        self.repos.add_form = ReposAddForm {
            active: true,
            step: 2,
            source: Some(params.source),
            method: Some(params.method),
            method_index,
            repo_id: params.repo_id,
            name,
            url: params.url,
            gpgcheck: params.gpgcheck,
            ..Default::default()
        };
        self.repos.add_form.revalidate();
        self.input_mode = InputMode::Editing;
    }

    pub(super) fn handle_confirm_key(&mut self, key: KeyEvent) -> Vec<Action> {
        match key.code {
            KeyCode::Enter | KeyCode::Char('y') => self
                .pending_confirm
                .take()
                .map(|p| vec![p.action])
                .unwrap_or_default(),
            KeyCode::Esc | KeyCode::Char('n' | 'q') => {
                if let Some(PendingConfirm {
                    action: Action::AddRepo(params),
                    ..
                }) = self.pending_confirm.take()
                {
                    self.reopen_add_form(params);
                }
                self.set_status(t!("tui.status.cancelled"));
                vec![]
            }
            _ => vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use pike_core::config::Config;
    use pike_core::package::{CleanupKind, RepoMethod};
    use ratatui::layout::Rect;

    use crate::tui::types::{ClickAction, ClickTarget, HitState, Tab, ViewState};

    fn app() -> (App, ViewState) {
        (
            App::new(Config::default(), Vec::new(), vec![SourceType::Dnf]),
            ViewState::new(false),
        )
    }

    fn key(app: &mut App, view: &mut ViewState, code: KeyCode) -> Vec<Action> {
        app.handle_key(KeyEvent::from(code), view)
    }

    fn install() -> Action {
        Action::InstallPackage("htop".into(), Some(SourceType::Dnf))
    }

    #[test]
    fn test_confirm_enter_executes() {
        let (mut app, mut view) = app();
        assert!(app.intercept(install()).is_none());
        assert!(app.pending_confirm.is_some());
        let actions = key(&mut app, &mut view, KeyCode::Enter);
        assert!(matches!(actions.as_slice(), [Action::InstallPackage(p, _)] if p == "htop"));
        assert!(app.pending_confirm.is_none());

        app.intercept(install());
        let actions = key(&mut app, &mut view, KeyCode::Char('y'));
        assert!(matches!(actions.as_slice(), [Action::InstallPackage(..)]));
    }

    #[test]
    fn test_confirm_esc_cancels() {
        for code in [KeyCode::Esc, KeyCode::Char('n'), KeyCode::Char('q')] {
            let (mut app, mut view) = app();
            app.intercept(install());
            assert!(key(&mut app, &mut view, code).is_empty());
            assert!(app.pending_confirm.is_none());
            assert!(app.running);
            assert_eq!(app.status_message, t!("tui.status.cancelled"));
        }
    }

    #[test]
    fn test_confirm_ignores_other_input() {
        let (mut app, mut view) = app();
        app.intercept(install());
        for code in [KeyCode::Char('2'), KeyCode::Char('j'), KeyCode::Tab] {
            assert!(key(&mut app, &mut view, code).is_empty());
        }
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        };
        assert!(
            app.handle_mouse(click, &HitState::default(), &mut view)
                .is_empty()
        );
        assert_eq!(app.tab, Tab::Search);
        assert!(app.pending_confirm.is_some());

        view.hover_row = Some(1);
        let moved = MouseEvent {
            kind: MouseEventKind::Moved,
            ..click
        };
        assert!(
            app.handle_mouse(moved, &HitState::default(), &mut view)
                .is_empty()
        );
        assert_eq!(view.hover_row, None);

        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(matches!(
            app.handle_key(ctrl_c, &mut view).as_slice(),
            [Action::Quit]
        ));
    }

    #[test]
    fn test_confirm_setting_off_executes_directly() {
        let (mut app, _) = app();
        app.config.display.confirm_actions = false;
        assert!(matches!(
            app.intercept(install()),
            Some(Action::InstallPackage(..))
        ));
        assert!(app.pending_confirm.is_none());
    }

    #[test]
    fn test_clean_preview_only_for_matching_items() {
        let (mut app, _) = app();
        let item = |name: &str| CleanupItem {
            source: SourceType::Dnf,
            kind: CleanupKind::Orphan,
            name: name.into(),
            version: "1".into(),
            size: Some(1024),
            arch: None,
        };
        assert!(app.pending_clean_items().is_none());
        app.intercept(Action::Clean(vec![item("a")]));
        assert_eq!(app.pending_clean_items(), Some([item("a")].as_slice()));
        let pending = app.pending_confirm.as_ref().map(|p| p.lines.clone());
        assert_eq!(pending, Some(vec!["[dnf] a 1".to_string()]));

        app.set_clean_preview(&[item("b")], vec![(SourceType::Dnf, Ok(vec!["x".into()]))]);
        assert!(
            app.pending_confirm
                .as_ref()
                .is_some_and(|p| p.preview.is_none())
        );

        app.set_clean_preview(&[item("a")], vec![(SourceType::Dnf, Ok(vec!["x".into()]))]);
        assert!(
            app.pending_confirm
                .as_ref()
                .is_some_and(|p| p.preview.is_some())
        );
    }

    #[test]
    fn test_cancel_add_repo_restores_form() {
        let (mut app, mut view) = app();
        app.tab = Tab::Repos;
        app.open_add_form();
        app.repos.add_form.url = "https://example.com/my.repo".into();
        app.repos.add_form.gpgcheck = false;
        let action = app.try_submit_add_repo().and_then(|a| app.intercept(a));
        assert!(action.is_none());
        assert!(!app.repos.add_form.active);

        key(&mut app, &mut view, KeyCode::Esc);
        let form = &app.repos.add_form;
        assert!(form.active);
        assert_eq!(form.step, 2);
        assert_eq!(form.url, "https://example.com/my.repo");
        assert!(!form.gpgcheck);
        assert_eq!(app.input_mode, InputMode::Editing);

        assert!(key(&mut app, &mut view, KeyCode::Enter).is_empty());
        let actions = key(&mut app, &mut view, KeyCode::Enter);
        assert!(matches!(
            actions.as_slice(),
            [Action::AddRepo(p)] if p.url == "https://example.com/my.repo" && !p.gpgcheck
        ));
    }

    #[test]
    fn test_cancel_add_repo_drops_derived_name() {
        let url = "https://dl.flathub.org/repo/flathub.flatpakrepo";
        for (typed, restored) in [("", ""), ("custom", "custom")] {
            let mut app = App::new(Config::default(), Vec::new(), vec![SourceType::Flatpak]);
            let mut view = ViewState::new(false);
            app.tab = Tab::Repos;
            app.open_add_form();
            app.repos.add_form.url = url.into();
            app.repos.add_form.name = typed.into();
            let action = app.try_submit_add_repo().and_then(|a| app.intercept(a));
            assert!(action.is_none());
            assert!(matches!(
                app.pending_confirm.as_ref().map(|p| &p.action),
                Some(Action::AddRepo(p)) if !p.name.is_empty()
            ));

            key(&mut app, &mut view, KeyCode::Esc);
            assert_eq!(app.repos.add_form.name, restored);
        }
    }

    #[test]
    fn test_add_repo_confirm_lists_url_and_repo_id() {
        let url = "https://download.docker.com/linux/fedora/docker-ce.repo";
        let confirm = |repo_id: &str, name: &str| {
            let (mut app, _) = app();
            app.intercept(Action::AddRepo(AddRepoParams {
                method: RepoMethod::RepoFile,
                repo_id: repo_id.into(),
                name: name.into(),
                url: url.into(),
                source: SourceType::Dnf,
                gpgcheck: true,
            }));
            app.pending_confirm.map(|p| (p.title, p.lines))
        };
        let url_line = t!("tui.confirm.url", url = url).to_string();
        let source_line = t!("tui.confirm.source", source = "dnf").to_string();

        assert_eq!(
            confirm("", ""),
            Some((
                t!("tui.confirm.add-repo-unnamed").to_string(),
                vec![url_line.clone(), source_line.clone()]
            ))
        );
        assert_eq!(
            confirm("docker", "Docker"),
            Some((
                t!("tui.confirm.add-repo", name = "Docker").to_string(),
                vec![
                    url_line,
                    t!("tui.confirm.repo-id", id = "docker").to_string(),
                    source_line
                ]
            ))
        );
    }

    #[test]
    fn test_repo_confirm_title_is_generic_with_id_in_body() {
        let confirm = |action: Action| {
            let (mut app, _) = app();
            app.intercept(action);
            app.pending_confirm.map(|p| (p.title, p.lines))
        };
        let repo_id_line = t!("tui.confirm.repo-id", id = "docker").to_string();
        let source_line = t!("tui.confirm.source", source = "dnf").to_string();

        assert_eq!(
            confirm(Action::ToggleRepo("docker".into(), true, SourceType::Dnf)),
            Some((
                t!("tui.confirm.enable-repo").to_string(),
                vec![repo_id_line.clone(), source_line.clone()]
            ))
        );
        assert_eq!(
            confirm(Action::ToggleRepo("docker".into(), false, SourceType::Dnf)),
            Some((
                t!("tui.confirm.disable-repo").to_string(),
                vec![repo_id_line.clone(), source_line.clone()]
            ))
        );
        assert_eq!(
            confirm(Action::DeleteRepo("docker".into(), SourceType::Dnf)),
            Some((
                t!("tui.confirm.delete-repo").to_string(),
                vec![repo_id_line, source_line]
            ))
        );
    }

    #[test]
    fn test_confirm_footer_click_confirms_or_cancels() {
        let hit = |code| HitState {
            click_targets: vec![ClickTarget {
                rect: Rect::new(0, 0, 5, 1),
                action: ClickAction::Key(code),
            }],
            table_zone: None,
        };
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 1,
            row: 0,
            modifiers: KeyModifiers::NONE,
        };

        let (mut app, mut view) = app();
        app.intercept(install());
        let actions = app.handle_mouse(click, &hit(KeyCode::Enter), &mut view);
        assert!(matches!(actions.as_slice(), [Action::InstallPackage(..)]));
        assert!(app.pending_confirm.is_none());

        app.intercept(install());
        assert!(
            app.handle_mouse(click, &hit(KeyCode::Esc), &mut view)
                .is_empty()
        );
        assert!(app.pending_confirm.is_none());
        assert_eq!(app.status_message, t!("tui.status.cancelled"));
    }
}
