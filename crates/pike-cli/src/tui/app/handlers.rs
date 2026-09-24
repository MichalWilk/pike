use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::widgets::TableState;

use super::App;
use crate::tui::types::{
    Action, CHECKBOX_WIDTH, ClickAction, FormField, HitState, InputMode, Tab, ViewState,
};
use pike_core::package::{Package, PackageUpdate, Repository};

fn selected_from<'a, T>(items: &'a [T], filtered: &[usize], table: &TableState) -> Option<&'a T> {
    let &real_idx = filtered.get(table.selected()?)?;
    items.get(real_idx)
}

fn set_cursor_pointer(view: &mut ViewState, clickable: bool) {
    if clickable != view.cursor_pointer {
        view.cursor_pointer = clickable;
        let shape = if clickable { "pointer" } else { "default" };
        let _ = std::io::Write::write_all(
            &mut std::io::stdout(),
            format!("\x1b]22;{shape}\x07").as_bytes(),
        );
    }
}

enum HitResult {
    Target(ClickAction),
    TableRow(usize),
    None,
}

impl App {
    pub(crate) fn handle_key(&mut self, key: KeyEvent, view: &mut ViewState) -> Vec<Action> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return vec![Action::Quit];
        }
        if self.pending_confirm.is_some() {
            return self.handle_confirm_key(key);
        }
        let actions = self.dispatch_key(key, view);
        self.intercept_all(actions)
    }

    fn dispatch_key(&mut self, key: KeyEvent, view: &mut ViewState) -> Vec<Action> {
        match self.input_mode {
            InputMode::Editing => self.handle_key_editing(key, view),
            InputMode::Normal => self.handle_key_normal(key, view),
        }
    }

    fn handle_key_editing(&mut self, key: KeyEvent, view: &mut ViewState) -> Vec<Action> {
        if self.tab == Tab::Repos && self.repos.add_form.active {
            return self.handle_repos_add_key(key);
        }
        if self.settings_input.is_some() {
            return self.handle_settings_input_key(key);
        }

        match key.code {
            KeyCode::Esc => {
                self.input_mode = InputMode::Normal;
                if let Some(filter) = self.active_filter_mut() {
                    filter.clear();
                }
                self.sync_current_tab_selection(view);
                vec![]
            }
            KeyCode::Enter => {
                self.input_mode = InputMode::Normal;
                if self.tab == Tab::Search && !self.search.input.is_empty() {
                    self.search.results.loading = true;
                    vec![Action::SearchSubmit(self.search.input.clone())]
                } else {
                    vec![]
                }
            }
            KeyCode::Char(c) => {
                if self.tab == Tab::Search {
                    self.search.input.push(c);
                } else if let Some(filter) = self.active_filter_mut() {
                    filter.push(c);
                    self.sync_current_tab_selection(view);
                }
                vec![]
            }
            KeyCode::Backspace => {
                if self.tab == Tab::Search {
                    self.search.input.pop();
                } else if let Some(filter) = self.active_filter_mut() {
                    filter.pop();
                    self.sync_current_tab_selection(view);
                }
                vec![]
            }
            _ => vec![],
        }
    }

    fn handle_key_normal(&mut self, key: KeyEvent, view: &mut ViewState) -> Vec<Action> {
        if self.tab == Tab::Repos && self.repos.add_form.active && self.repos.add_form.step == 0 {
            return self.handle_repos_picker_key(key);
        }

        match key.code {
            KeyCode::Char('q') => vec![Action::Quit],

            KeyCode::Char(c @ '1'..='5') => {
                let idx = (c as usize) - ('1' as usize);
                self.switch_tab(Tab::ALL[idx]);
                vec![]
            }
            KeyCode::Char('9') => {
                self.switch_tab(Tab::Settings);
                vec![]
            }
            KeyCode::Char('0') => {
                self.switch_tab(Tab::About);
                vec![]
            }
            KeyCode::Tab => {
                self.switch_tab(self.tab.next());
                vec![]
            }
            KeyCode::BackTab => {
                self.switch_tab(self.tab.prev());
                vec![]
            }

            KeyCode::Down | KeyCode::Char('j') => {
                self.move_selection(1, view);
                vec![]
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_selection(-1, view);
                vec![]
            }

            _ => self.handle_tab_key(key, view),
        }
    }

    fn handle_tab_key(&mut self, key: KeyEvent, view: &mut ViewState) -> Vec<Action> {
        match self.tab {
            Tab::Search => self.handle_search_key(key, view),
            Tab::Installed => self.handle_installed_key(key, view),
            Tab::Updates => self.handle_updates_key(key, view),
            Tab::Repos => self.handle_repos_key(key, view),
            Tab::Cleanup => self.handle_cleanup_key(key, view),
            Tab::Settings => self.handle_settings_key(key, view),
            Tab::About => match key.code {
                KeyCode::Enter => {
                    let idx = view.about_table.selected().unwrap_or(0);
                    vec![Action::OpenUrl(
                        crate::tui::ui::about::ABOUT_URLS[idx].into(),
                    )]
                }
                _ => vec![],
            },
        }
    }

    fn handle_search_key(&mut self, key: KeyEvent, view: &mut ViewState) -> Vec<Action> {
        match key.code {
            KeyCode::Char('/') => {
                self.input_mode = InputMode::Editing;
                vec![]
            }
            KeyCode::Char('s') => {
                self.cycle_source_filter(view);
                vec![]
            }
            KeyCode::Char('i') => {
                if let Some(pkg) = self.selected_search_package(view)
                    && !self.is_installed(&pkg.name, pkg.source)
                {
                    return vec![Action::InstallPackage(pkg.name.clone(), Some(pkg.source))];
                }
                vec![]
            }
            KeyCode::Char('d') => {
                if let Some(pkg) = self.selected_search_package(view)
                    && self.is_installed(&pkg.name, pkg.source)
                {
                    return vec![Action::RemovePackage(pkg.name.clone(), Some(pkg.source))];
                }
                vec![]
            }
            KeyCode::Char('r') if !self.search.input.is_empty() => {
                self.search.results.loading = true;
                vec![Action::SearchSubmit(self.search.input.clone())]
            }
            _ => vec![],
        }
    }

    fn handle_installed_key(&mut self, key: KeyEvent, view: &mut ViewState) -> Vec<Action> {
        match key.code {
            KeyCode::Char('/') => {
                self.input_mode = InputMode::Editing;
                vec![]
            }
            KeyCode::Char('s') => {
                self.cycle_source_filter(view);
                vec![]
            }
            KeyCode::Char('d') => {
                if let Some(pkg) = self.selected_installed_package(view) {
                    return vec![Action::RemovePackage(pkg.name.clone(), Some(pkg.source))];
                }
                vec![]
            }
            KeyCode::Char('r') => vec![Action::RefreshInstalled],
            _ => vec![],
        }
    }

    fn handle_updates_key(&mut self, key: KeyEvent, view: &mut ViewState) -> Vec<Action> {
        match key.code {
            KeyCode::Char('/') => {
                self.input_mode = InputMode::Editing;
                vec![]
            }
            KeyCode::Char('s') => {
                self.cycle_source_filter(view);
                vec![]
            }
            KeyCode::Char('u') => {
                if let Some(u) = self.selected_update(view) {
                    return vec![Action::UpdatePackage(u.name.clone(), u.source)];
                }
                vec![]
            }
            KeyCode::Char('U') if !self.updates.items.is_empty() => {
                let pkgs: Vec<_> = self
                    .updates_filtered_indices()
                    .iter()
                    .map(|&i| {
                        let u = &self.updates.items[i];
                        (u.name.clone(), u.source)
                    })
                    .collect();
                if pkgs.is_empty() {
                    vec![]
                } else {
                    vec![Action::UpdateAll(pkgs)]
                }
            }
            KeyCode::Char('r') => vec![Action::RefreshUpdates],
            _ => vec![],
        }
    }

    fn handle_repos_key(&mut self, key: KeyEvent, view: &mut ViewState) -> Vec<Action> {
        match key.code {
            KeyCode::Char('/') => {
                self.input_mode = InputMode::Editing;
                vec![]
            }
            KeyCode::Char('e') => {
                if let Some(repo) = self.selected_repo(view) {
                    return vec![Action::ToggleRepo(
                        repo.id.clone(),
                        !repo.enabled,
                        repo.source,
                    )];
                }
                vec![]
            }
            KeyCode::Char('a') => {
                self.open_add_form();
                vec![]
            }
            KeyCode::Char('d') => {
                if let Some(repo) = self.selected_repo(view) {
                    return vec![Action::DeleteRepo(repo.id.clone(), repo.source)];
                }
                vec![]
            }
            KeyCode::Char('s') => {
                self.cycle_source_filter(view);
                vec![]
            }
            KeyCode::Char('r') => vec![Action::RefreshRepos],
            _ => vec![],
        }
    }

    fn handle_cleanup_key(&mut self, key: KeyEvent, view: &mut ViewState) -> Vec<Action> {
        self.last_clean_ok = None;
        match key.code {
            KeyCode::Char('/') => {
                self.input_mode = InputMode::Editing;
                vec![]
            }
            KeyCode::Char('s') => {
                self.cycle_source_filter(view);
                vec![]
            }
            KeyCode::Char(' ' | 'e') => {
                self.toggle_selected_cleanup(view);
                vec![]
            }
            KeyCode::Char('a') => {
                let filtered = self.cleanup_filtered_indices();
                let all_selected = filtered.iter().all(|i| self.cleanup_selected.contains(i));
                for i in filtered {
                    if all_selected {
                        self.cleanup_selected.remove(&i);
                    } else {
                        self.cleanup_selected.insert(i);
                    }
                }
                vec![]
            }
            KeyCode::Char('c') => {
                let items: Vec<_> = self.selected_cleanup_items().cloned().collect();
                if self.cleanup.loading || items.is_empty() {
                    vec![]
                } else {
                    vec![Action::Clean(items)]
                }
            }
            KeyCode::Char('r') => vec![Action::RefreshCleanup],
            _ => vec![],
        }
    }

    fn toggle_selected_cleanup(&mut self, view: &ViewState) {
        self.last_clean_ok = None;
        let filtered = self.cleanup_filtered_indices();
        if let Some(&real) = view.cleanup_table.selected().and_then(|s| filtered.get(s)) {
            self.toggle_cleanup_selection(real);
        }
    }

    pub(crate) fn selected_search_package(&self, view: &ViewState) -> Option<&Package> {
        selected_from(
            &self.search.results.items,
            &self.search_filtered_indices(),
            &view.search_table,
        )
    }

    fn selected_installed_package(&self, view: &ViewState) -> Option<&Package> {
        selected_from(
            &self.installed.items,
            &self.installed_filtered_indices(),
            &view.installed_table,
        )
    }

    fn selected_update(&self, view: &ViewState) -> Option<&PackageUpdate> {
        selected_from(
            &self.updates.items,
            &self.updates_filtered_indices(),
            &view.updates_table,
        )
    }

    fn selected_repo(&self, view: &ViewState) -> Option<&Repository> {
        selected_from(
            &self.repos.list.items,
            &self.repos_filtered_indices(),
            &view.repos_table,
        )
    }

    fn handle_repos_picker_key(&mut self, key: KeyEvent) -> Vec<Action> {
        let entries = self.picker_entries();
        if entries.is_empty() {
            return vec![];
        }
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.repos.add_form.method_index =
                    (self.repos.add_form.method_index + 1) % entries.len();
                self.apply_picker_selection(&entries);
                vec![]
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.repos.add_form.method_index = if self.repos.add_form.method_index == 0 {
                    entries.len() - 1
                } else {
                    self.repos.add_form.method_index - 1
                };
                self.apply_picker_selection(&entries);
                vec![]
            }
            KeyCode::Char('e') => {
                self.apply_picker_selection(&entries);
                self.repos.add_form.step = 2;
                self.repos.add_form.field = 0;
                self.input_mode = InputMode::Editing;
                vec![]
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                self.repos.add_form.active = false;
                vec![]
            }
            _ => vec![],
        }
    }

    fn handle_repos_add_key(&mut self, key: KeyEvent) -> Vec<Action> {
        let fields = self.repos.add_form.fields();
        let field_count = fields.len();
        let active_field = self.repos.add_form.active_field();

        match key.code {
            KeyCode::Esc => {
                self.repos.add_form.step = 0;
                self.repos.add_form.repo_id.clear();
                self.repos.add_form.name.clear();
                self.repos.add_form.url.clear();
                self.repos.add_form.gpgcheck = true;
                self.input_mode = InputMode::Normal;
                vec![]
            }
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Down | KeyCode::Up => {
                if field_count > 1 {
                    let field = self.repos.add_form.field as usize;
                    let forward = matches!(key.code, KeyCode::Tab | KeyCode::Down);
                    self.repos.add_form.field = if forward {
                        ((field + 1) % field_count) as u8
                    } else {
                        ((field + field_count - 1) % field_count) as u8
                    };
                }
                vec![]
            }
            KeyCode::Char('e') if active_field == Some(FormField::GpgCheck) => {
                self.repos.add_form.gpgcheck = !self.repos.add_form.gpgcheck;
                vec![]
            }
            KeyCode::Enter => {
                if !self.repos.add_form.validation_error.is_empty() {
                    return vec![];
                }
                match self.try_submit_add_repo() {
                    Some(action) => vec![action],
                    None => vec![],
                }
            }
            KeyCode::Char(c) => {
                let Some(s) = active_field.and_then(|f| self.repos.add_form.field_text_mut(f))
                else {
                    return vec![];
                };
                s.push(c);
                self.repos.add_form.revalidate();
                vec![]
            }
            KeyCode::Backspace => {
                let Some(s) = active_field.and_then(|f| self.repos.add_form.field_text_mut(f))
                else {
                    return vec![];
                };
                s.pop();
                self.repos.add_form.revalidate();
                vec![]
            }
            _ => vec![],
        }
    }

    fn handle_settings_key(&mut self, key: KeyEvent, view: &mut ViewState) -> Vec<Action> {
        match key.code {
            KeyCode::Char('e') => {
                if self.activate_selected_setting(view) {
                    vec![Action::SaveSettings]
                } else {
                    vec![]
                }
            }
            _ => vec![],
        }
    }

    pub(crate) fn handle_mouse(
        &mut self,
        event: MouseEvent,
        hit: &HitState,
        view: &mut ViewState,
    ) -> Vec<Action> {
        if self.pending_confirm.is_some() {
            let target = match self.hit_test(view, hit, event.column, event.row) {
                HitResult::Target(ClickAction::Key(code)) => Some(code),
                _ => None,
            };
            return match (event.kind, target) {
                (MouseEventKind::Down(MouseButton::Left), Some(code)) => {
                    self.handle_confirm_key(KeyEvent::from(code))
                }
                (MouseEventKind::Moved, _) => {
                    view.hover_row = None;
                    set_cursor_pointer(view, target.is_some());
                    vec![]
                }
                _ => vec![],
            };
        }
        match event.kind {
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp
                if self.settings_input.is_some() =>
            {
                vec![]
            }
            MouseEventKind::ScrollDown => {
                self.move_selection(1, view);
                vec![]
            }
            MouseEventKind::ScrollUp => {
                self.move_selection(-1, view);
                vec![]
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let actions = self.handle_click(hit, event.column, event.row, view);
                self.intercept_all(actions)
            }
            MouseEventKind::Moved if self.settings_input.is_some() => {
                view.hover_row = None;
                let on_button = matches!(
                    self.hit_test(view, hit, event.column, event.row),
                    HitResult::Target(ClickAction::Key(_))
                );
                set_cursor_pointer(view, on_button);
                vec![]
            }
            MouseEventKind::Moved => {
                self.update_hover(view, hit, event.column, event.row);
                self.update_cursor_shape(view, hit, event.column, event.row);
                vec![]
            }
            _ => vec![],
        }
    }

    fn handle_click(
        &mut self,
        hit: &HitState,
        col: u16,
        row: u16,
        view: &mut ViewState,
    ) -> Vec<Action> {
        match self.hit_test(view, hit, col, row) {
            HitResult::Target(ClickAction::SwitchTab(tab)) => {
                self.switch_tab(tab);
                vec![]
            }
            HitResult::Target(ClickAction::Key(code)) => {
                self.dispatch_key(KeyEvent::from(code), view)
            }
            HitResult::Target(ClickAction::AboutLink(i)) => {
                match crate::tui::ui::about::ABOUT_URLS.get(i) {
                    Some(url) => {
                        view.about_table.select(Some(i));
                        vec![Action::OpenUrl((*url).into())]
                    }
                    None => vec![],
                }
            }
            HitResult::TableRow(_) if self.settings_input.is_some() => vec![],
            HitResult::TableRow(idx) => {
                view.table_for(self.tab).select(Some(idx));
                if self.tab == Tab::Cleanup
                    && let Some(zone) = &hit.table_zone
                    && col < zone.x_start + CHECKBOX_WIDTH
                {
                    self.toggle_selected_cleanup(view);
                }
                vec![]
            }
            HitResult::None => vec![],
        }
    }

    fn hit_test(&self, view: &ViewState, hit: &HitState, col: u16, row: u16) -> HitResult {
        for target in &hit.click_targets {
            let r = target.rect;
            if col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height {
                return HitResult::Target(target.action);
            }
        }
        if let Some(ref zone) = hit.table_zone
            && col >= zone.x_start
            && col < zone.x_start + zone.width
            && row >= zone.y_start
            && row < zone.y_start + zone.visible_rows
        {
            let actual_idx = view.table_offset(self.tab) + (row - zone.y_start) as usize;
            if actual_idx < zone.item_count {
                return HitResult::TableRow(actual_idx);
            }
        }
        HitResult::None
    }

    fn update_cursor_shape(&self, view: &mut ViewState, hit: &HitState, col: u16, row: u16) {
        let clickable = !matches!(self.hit_test(view, hit, col, row), HitResult::None);
        set_cursor_pointer(view, clickable);
    }

    fn update_hover(&self, view: &mut ViewState, hit: &HitState, col: u16, row: u16) {
        view.hover_row = match self.hit_test(view, hit, col, row) {
            HitResult::TableRow(idx) => Some(idx),
            _ => None,
        };
    }

    fn move_selection(&self, delta: i32, view: &mut ViewState) {
        let max = match self.tab {
            Tab::Search => self.search_filtered_indices().len(),
            Tab::Installed => self.installed_filtered_indices().len(),
            Tab::Updates => self.updates_filtered_indices().len(),
            Tab::Repos => self.repos_filtered_indices().len(),
            Tab::Cleanup => self.cleanup_filtered_indices().len(),
            Tab::Settings => self.settings_count(),
            Tab::About => crate::tui::ui::about::ABOUT_URLS.len(),
        };
        if max == 0 {
            return;
        }
        let state = view.table_for(self.tab);
        let current = state.selected().unwrap_or(0);
        let new = (current as i32 + delta).clamp(0, max as i32 - 1) as usize;
        state.select(Some(new));

        if self.tab == Tab::Settings {
            self.settings_skip_groups(delta, view);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    use pike_core::config::Config;
    use pike_core::package::{CleanupItem, CleanupKind, CleanupScan, SourceType};

    use crate::tui::types::TableClickZone;

    fn cleanup_app() -> (App, ViewState) {
        let mut app = App::new(
            Config::default(),
            Vec::new(),
            vec![SourceType::Dnf, SourceType::Flatpak],
        );
        app.tab = Tab::Cleanup;
        let mut view = ViewState::new(false);
        let item = |source, kind, name: &str| CleanupItem {
            source,
            kind,
            name: name.into(),
            version: String::new(),
            size: Some(10),
            arch: None,
        };
        let scan = CleanupScan {
            items: vec![
                item(SourceType::Dnf, CleanupKind::Orphan, "libfoo.x86_64"),
                item(SourceType::Flatpak, CleanupKind::UnusedRuntime, "org.a"),
                item(SourceType::Dnf, CleanupKind::Cache, "/var/cache/dnf"),
                item(SourceType::Flatpak, CleanupKind::UnusedRuntime, "org.b"),
            ],
            failed: Vec::new(),
        };
        let keep = app.config.cleanup.keep_kernels();
        app.set_cleanup(scan, keep, &mut view);
        (app, view)
    }

    fn press(app: &mut App, view: &mut ViewState, c: char) -> Vec<Action> {
        app.handle_key(KeyEvent::from(KeyCode::Char(c)), view)
    }

    #[test]
    fn test_cleanup_key_ignored_while_loading() {
        let (mut app, mut view) = cleanup_app();
        app.cleanup.loading = true;
        assert!(press(&mut app, &mut view, 'c').is_empty());

        app.cleanup.loading = false;
        assert!(press(&mut app, &mut view, 'c').is_empty());
        assert_eq!(app.pending_clean_items().map(<[_]>::len), Some(4));

        let actions = app.handle_key(KeyEvent::from(KeyCode::Enter), &mut view);
        assert!(matches!(actions.as_slice(), [Action::Clean(items)] if items.len() == 4));
        assert!(app.pending_confirm.is_none());
    }

    #[test]
    fn test_cleanup_filtered_row_maps_to_real_item() {
        let (mut app, mut view) = cleanup_app();
        app.cleanup_selected.clear();
        app.cleanup.source_filter = Some(SourceType::Flatpak);
        view.cleanup_table.select(Some(0));
        press(&mut app, &mut view, ' ');
        assert_eq!(app.cleanup_selected, HashSet::from([1]));

        press(&mut app, &mut view, 'a');
        assert_eq!(app.cleanup_selected, HashSet::from([1, 3]));

        press(&mut app, &mut view, 'a');
        assert!(app.cleanup_selected.is_empty());
    }

    #[test]
    fn test_cleanup_action_clears_clean_result() {
        let (mut app, mut view) = cleanup_app();
        app.last_clean_ok = Some(false);
        press(&mut app, &mut view, 'e');
        assert_eq!(app.last_clean_ok, None);
        assert!(!app.cleanup_selected.contains(&0));

        app.last_clean_ok = Some(true);
        press(&mut app, &mut view, '1');
        assert_eq!(app.last_clean_ok, None);
    }

    #[test]
    fn test_cleanup_checkbox_click_toggles_filtered_row() {
        let (mut app, mut view) = cleanup_app();
        app.cleanup_selected.clear();
        app.cleanup.source_filter = Some(SourceType::Flatpak);
        let hit = HitState {
            click_targets: Vec::new(),
            table_zone: Some(TableClickZone {
                y_start: 5,
                x_start: 2,
                width: 80,
                visible_rows: 10,
                item_count: 2,
            }),
        };
        let click = |column| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row: 6,
            modifiers: KeyModifiers::NONE,
        };

        app.handle_mouse(click(2 + CHECKBOX_WIDTH), &hit, &mut view);
        assert_eq!(view.cleanup_table.selected(), Some(1));
        assert!(app.cleanup_selected.is_empty());

        app.handle_mouse(click(2 + CHECKBOX_WIDTH - 1), &hit, &mut view);
        assert_eq!(app.cleanup_selected, HashSet::from([3]));

        app.input_mode = InputMode::Editing;
        app.cleanup.filter = "org".into();
        app.last_clean_ok = Some(true);
        app.handle_mouse(click(2 + CHECKBOX_WIDTH - 1), &hit, &mut view);
        assert!(app.cleanup_selected.is_empty());
        assert_eq!(app.cleanup.filter, "org");
        assert_eq!(app.last_clean_ok, None);
    }

    #[test]
    fn test_settings_input_ignores_scroll_and_row_clicks() {
        let mut app = App::new(Config::default(), Vec::new(), vec![SourceType::Dnf]);
        app.tab = Tab::Settings;
        app.ensure_settings_cache();
        let mut view = ViewState::new(false);
        view.settings_table.select(Some(2));
        app.settings_input = Some("#".into());
        app.input_mode = InputMode::Editing;
        let target = |y, action| crate::tui::types::ClickTarget {
            rect: ratatui::layout::Rect::new(0, y, 10, 1),
            action,
        };
        let hit = HitState {
            click_targets: vec![
                target(0, ClickAction::SwitchTab(Tab::Search)),
                target(20, ClickAction::Key(KeyCode::Esc)),
            ],
            table_zone: Some(TableClickZone {
                y_start: 5,
                x_start: 2,
                width: 80,
                visible_rows: 10,
                item_count: app.settings_count(),
            }),
        };
        let event_at = |kind, row| MouseEvent {
            kind,
            column: 5,
            row,
            modifiers: KeyModifiers::NONE,
        };
        let event = |kind| event_at(kind, 8);
        for kind in [
            MouseEventKind::ScrollDown,
            MouseEventKind::ScrollUp,
            MouseEventKind::Down(MouseButton::Left),
        ] {
            assert!(app.handle_mouse(event(kind), &hit, &mut view).is_empty());
            assert_eq!(view.settings_table.selected(), Some(2));
        }
        view.hover_row = Some(3);
        app.handle_mouse(event(MouseEventKind::Moved), &hit, &mut view);
        assert_eq!(view.hover_row, None);
        assert!(!view.cursor_pointer);
        app.handle_mouse(event_at(MouseEventKind::Moved, 20), &hit, &mut view);
        assert!(view.cursor_pointer);
        app.handle_mouse(event_at(MouseEventKind::Moved, 0), &hit, &mut view);
        assert!(!view.cursor_pointer);
        assert_eq!(app.settings_input.as_deref(), Some("#"));
        assert_eq!(app.input_mode, InputMode::Editing);
    }

    #[test]
    fn test_about_link_click_opens_url_and_selects() {
        let mut app = App::new(Config::default(), Vec::new(), vec![SourceType::Dnf]);
        app.tab = Tab::About;
        let mut view = ViewState::new(false);
        let hit = HitState {
            click_targets: vec![
                crate::tui::types::ClickTarget {
                    rect: ratatui::layout::Rect::new(10, 6, 20, 1),
                    action: ClickAction::AboutLink(1),
                },
                crate::tui::types::ClickTarget {
                    rect: ratatui::layout::Rect::new(10, 7, 20, 1),
                    action: ClickAction::AboutLink(9),
                },
            ],
            table_zone: None,
        };
        let click_at = |row| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 15,
            row,
            modifiers: KeyModifiers::NONE,
        };
        let actions = app.handle_mouse(click_at(6), &hit, &mut view);
        assert!(matches!(
            actions.as_slice(),
            [Action::OpenUrl(url)] if url == crate::tui::ui::about::ABOUT_URLS[1]
        ));
        assert_eq!(view.about_table.selected(), Some(1));
        assert!(app.handle_mouse(click_at(7), &hit, &mut view).is_empty());
        assert_eq!(view.about_table.selected(), Some(1));
    }
}
