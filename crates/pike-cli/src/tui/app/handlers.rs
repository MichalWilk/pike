use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::widgets::TableState;

use super::App;
use crate::tui::types::{Action, ClickAction, FormField, HitState, InputMode, Tab, ViewState};
use pike_core::package::{Package, PackageUpdate, Repository};

fn selected_from<'a, T>(items: &'a [T], filtered: &[usize], table: &TableState) -> Option<&'a T> {
    let &real_idx = filtered.get(table.selected()?)?;
    items.get(real_idx)
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

        match self.input_mode {
            InputMode::Editing => self.handle_key_editing(key, view),
            InputMode::Normal => self.handle_key_normal(key, view),
        }
    }

    fn handle_key_editing(&mut self, key: KeyEvent, view: &mut ViewState) -> Vec<Action> {
        if self.tab == Tab::Repos && self.repos.add_form.active {
            return self.handle_repos_add_key(key);
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

            KeyCode::Char(c @ '1'..='4') => {
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
            KeyCode::Char('A') => vec![Action::Autoremove],
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
        match event.kind {
            MouseEventKind::ScrollDown => {
                self.move_selection(1, view);
                vec![]
            }
            MouseEventKind::ScrollUp => {
                self.move_selection(-1, view);
                vec![]
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.handle_click(hit, event.column, event.row, view)
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
                let key = KeyEvent::from(code);
                self.handle_key(key, view)
            }
            HitResult::TableRow(idx) => {
                view.table_for(self.tab).select(Some(idx));
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
        if clickable != view.cursor_pointer {
            view.cursor_pointer = clickable;
            let shape = if clickable { "pointer" } else { "default" };
            let _ = std::io::Write::write_all(
                &mut std::io::stdout(),
                format!("\x1b]22;{shape}\x07").as_bytes(),
            );
        }
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
