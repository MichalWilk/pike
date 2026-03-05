mod handlers;
mod settings;

use std::collections::{HashMap, HashSet};

use pike_core::config::Config;
use pike_core::package::{Package, PackageUpdate, RepoMethod, Repository, SourceType};
use ratatui::widgets::TableState;
use rust_i18n::t;

pub use super::types::{Action, InputMode, Tab};
use super::types::{AddRepoParams, FormField, SettingsRow, ViewState};

fn name_from_url(url: &str) -> String {
    let stripped = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url)
        .trim_end_matches('/');
    let last_segment = stripped.rsplit('/').next().unwrap_or(stripped);
    let name = last_segment.split('.').next().unwrap_or(last_segment);
    if name.is_empty() {
        stripped
            .split('/')
            .next()
            .unwrap_or("repo")
            .split('.')
            .next()
            .unwrap_or("repo")
            .to_string()
    } else {
        name.to_string()
    }
}

pub(crate) struct ListTabState<T> {
    pub(crate) items: Vec<T>,
    pub(crate) filter: String,
    pub(crate) source_filter: Option<SourceType>,
    pub(crate) loading: bool,
    pub(crate) loaded: bool,
}

impl<T> ListTabState<T> {
    fn new() -> Self {
        Self {
            items: Vec::new(),
            filter: String::new(),
            source_filter: None,
            loading: false,
            loaded: false,
        }
    }

    pub(crate) fn filtered_indices(
        &self,
        source_of: impl Fn(&T) -> SourceType,
        matches_query: impl Fn(&T, &str) -> bool,
    ) -> Vec<usize> {
        let query_lower = self.filter.to_lowercase();
        self.items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                if let Some(st) = self.source_filter
                    && source_of(item) != st
                {
                    return false;
                }
                query_lower.is_empty() || matches_query(item, &query_lower)
            })
            .map(|(i, _)| i)
            .collect()
    }

    fn sync_selection(
        &self,
        table: &mut TableState,
        source_of: impl Fn(&T) -> SourceType,
        matches_query: impl Fn(&T, &str) -> bool,
    ) {
        let count = self.filtered_indices(source_of, matches_query).len();
        table.select(if count == 0 { None } else { Some(0) });
    }

    fn cycle_source_filter(
        &mut self,
        table: &mut TableState,
        source_of: impl Fn(&T) -> SourceType,
        matches_query: impl Fn(&T, &str) -> bool,
    ) {
        self.source_filter = match self.source_filter {
            None => Some(SourceType::ALL[0]),
            Some(st) => {
                let pos = SourceType::ALL.iter().position(|&s| s == st).unwrap_or(0);
                SourceType::ALL.get(pos + 1).copied()
            }
        };
        self.sync_selection(table, source_of, matches_query);
    }
}

pub(crate) struct SearchTab {
    pub(crate) input: String,
    pub(crate) results: ListTabState<Package>,
}

#[derive(Default)]
pub(crate) struct ReposAddForm {
    pub(crate) active: bool,
    pub(crate) step: u8,
    pub(crate) source: Option<SourceType>,
    pub(crate) method: Option<RepoMethod>,
    pub(crate) method_index: usize,
    pub(crate) repo_id: String,
    pub(crate) name: String,
    pub(crate) url: String,
    pub(crate) field: u8,
    pub(crate) gpgcheck: bool,
    pub(crate) validation_error: String,
}

impl ReposAddForm {
    pub(crate) fn fields(&self) -> Vec<FormField> {
        let method = self.method.unwrap_or(RepoMethod::RemoteAdd);
        let mut fields = Vec::new();
        if method.has_repo_id() {
            fields.push(FormField::RepoId);
        }
        if method.has_display_name() || method.needs_name() {
            fields.push(FormField::Name);
        }
        fields.push(FormField::Url);
        if method.has_gpgcheck() {
            fields.push(FormField::GpgCheck);
        }
        fields
    }

    pub(crate) fn active_field(&self) -> Option<FormField> {
        self.fields().get(self.field as usize).copied()
    }

    pub(crate) fn field_text(&self, field: FormField) -> Option<&str> {
        match field {
            FormField::RepoId => Some(&self.repo_id),
            FormField::Name => Some(&self.name),
            FormField::Url => Some(&self.url),
            FormField::GpgCheck => None,
        }
    }

    pub(crate) fn field_text_mut(&mut self, field: FormField) -> Option<&mut String> {
        match field {
            FormField::RepoId => Some(&mut self.repo_id),
            FormField::Name => Some(&mut self.name),
            FormField::Url => Some(&mut self.url),
            FormField::GpgCheck => None,
        }
    }

    pub(crate) fn revalidate(&mut self) {
        let method = self.method.unwrap_or(RepoMethod::RemoteAdd);
        let name = if self.name.is_empty() && method.needs_name() {
            name_from_url(&self.url)
        } else {
            self.name.clone()
        };
        match pike_core::manager::validate_repo_input(method, &self.repo_id, &name, &self.url) {
            Ok(()) => self.validation_error.clear(),
            Err(e) => self.validation_error = e.to_string(),
        }
    }
}

pub(crate) struct ReposTab {
    pub(crate) list: ListTabState<Repository>,
    pub(crate) add_form: ReposAddForm,
}

pub(crate) struct App {
    pub(crate) running: bool,
    pub(crate) tab: Tab,
    pub(crate) input_mode: InputMode,

    pub(crate) search: SearchTab,
    pub(crate) installed: ListTabState<Package>,
    pub(crate) updates: ListTabState<PackageUpdate>,
    pub(crate) repos: ReposTab,

    pub(crate) config: Config,
    pub(crate) status_message: String,
    pub(crate) active_sources: Vec<SourceType>,
    installed_set: HashMap<SourceType, HashSet<String>>,
    pub(super) cached_settings_layout: Option<Vec<SettingsRow>>,
    pub(crate) daemon_running: bool,
}

fn pkg_source(pkg: &Package) -> SourceType {
    pkg.source
}

fn pkg_matches(pkg: &Package, q: &str) -> bool {
    pkg.name.to_lowercase().contains(q)
        || pkg
            .description
            .as_deref()
            .is_some_and(|d| d.to_lowercase().contains(q))
}

fn update_source(u: &PackageUpdate) -> SourceType {
    u.source
}

fn update_matches(u: &PackageUpdate, q: &str) -> bool {
    u.name.to_lowercase().contains(q)
}

fn repo_source(r: &Repository) -> SourceType {
    r.source
}

fn repo_matches(r: &Repository, q: &str) -> bool {
    r.id.to_lowercase().contains(q) || r.name.to_lowercase().contains(q)
}

impl App {
    pub(crate) fn new(
        config: Config,
        updates: Vec<PackageUpdate>,
        active_sources: Vec<SourceType>,
    ) -> Self {
        Self {
            running: true,
            tab: Tab::Search,
            input_mode: InputMode::Normal,
            search: SearchTab {
                input: String::new(),
                results: ListTabState::new(),
            },
            installed: ListTabState::new(),
            updates: ListTabState {
                items: updates,
                filter: String::new(),
                source_filter: None,
                loading: false,
                loaded: true,
            },
            repos: ReposTab {
                list: ListTabState::new(),
                add_form: ReposAddForm::default(),
            },
            config,
            status_message: String::new(),
            active_sources,
            installed_set: HashMap::new(),
            cached_settings_layout: None,
            daemon_running: false,
        }
    }

    pub(crate) fn has_source(&self, st: SourceType) -> bool {
        self.active_sources.contains(&st)
    }

    pub(crate) fn missing_backends(&self) -> Vec<&'static str> {
        SourceType::ALL
            .iter()
            .filter(|&&st| self.config.sources.enabled(st) && !self.has_source(st))
            .map(|st| st.binary_name())
            .collect()
    }

    pub(crate) fn set_search_results(&mut self, results: Vec<Package>, view: &mut ViewState) {
        self.search.results.items = results;
        self.search
            .results
            .sync_selection(&mut view.search_table, pkg_source, |_, _| true);
        self.search.results.loading = false;
        self.status_message = t!(
            "tui.status.results",
            count = self.search.results.items.len()
        )
        .to_string();
    }

    pub(crate) fn search_filtered_indices(&self) -> Vec<usize> {
        self.search
            .results
            .filtered_indices(pkg_source, |_, _| true)
    }

    pub(crate) fn set_installed(&mut self, packages: Vec<Package>, view: &mut ViewState) {
        let total = packages.len();
        self.rebuild_installed_set(&packages);
        self.installed.items = packages;
        self.installed.filter.clear();
        self.installed
            .sync_selection(&mut view.installed_table, pkg_source, pkg_matches);
        self.installed.loading = false;
        self.installed.loaded = true;
        self.status_message = t!("tui.status.installed-count", count = total).to_string();
    }

    fn rebuild_installed_set(&mut self, packages: &[Package]) {
        self.installed_set.clear();
        for p in packages {
            self.installed_set
                .entry(p.source)
                .or_default()
                .insert(p.name.clone());
        }
    }

    pub(crate) fn is_installed(&self, name: &str, source: SourceType) -> bool {
        self.installed_set
            .get(&source)
            .is_some_and(|s| s.contains(name))
    }

    pub(crate) fn mark_installed(&mut self, name: &str, source: SourceType) {
        self.installed_set
            .entry(source)
            .or_default()
            .insert(name.to_string());
    }

    pub(crate) fn mark_removed(&mut self, name: &str, source: SourceType) {
        if let Some(set) = self.installed_set.get_mut(&source) {
            set.remove(name);
        }
    }

    pub(crate) fn installed_filtered_indices(&self) -> Vec<usize> {
        self.installed.filtered_indices(pkg_source, pkg_matches)
    }

    pub(crate) fn set_updates(&mut self, updates: Vec<PackageUpdate>, view: &mut ViewState) {
        let total = updates.len();
        self.updates.items = updates;
        self.updates.filter.clear();
        self.updates
            .sync_selection(&mut view.updates_table, update_source, update_matches);
        self.status_message = t!("tui.status.updates-loaded", count = total).to_string();
    }

    pub(crate) fn updates_filtered_indices(&self) -> Vec<usize> {
        self.updates.filtered_indices(update_source, update_matches)
    }

    pub(crate) fn set_status(&mut self, msg: impl Into<String>) {
        self.status_message = msg.into();
    }

    fn switch_tab(&mut self, tab: Tab) {
        self.tab = tab;
        self.status_message.clear();
        if tab == Tab::Settings {
            self.refresh_daemon_status();
        }
    }

    fn apply_picker_selection(&mut self, entries: &[(SourceType, RepoMethod)]) {
        let (source, method) = entries[self.repos.add_form.method_index];
        self.repos.add_form.source = Some(source);
        self.repos.add_form.method = Some(method);
    }

    pub(crate) fn picker_entries(&self) -> Vec<(SourceType, RepoMethod)> {
        self.active_sources
            .iter()
            .flat_map(|&st| RepoMethod::methods_for(st).iter().map(move |&m| (st, m)))
            .collect()
    }

    pub(crate) fn open_add_form(&mut self) {
        self.repos.add_form = ReposAddForm {
            active: true,
            gpgcheck: true,
            ..Default::default()
        };
        let entries = self.picker_entries();
        if entries.len() == 1 {
            let (source, method) = entries[0];
            self.repos.add_form.source = Some(source);
            self.repos.add_form.method = Some(method);
            self.repos.add_form.step = 2;
            self.repos.add_form.field = 0;
            self.input_mode = InputMode::Editing;
        } else {
            let (source, method) = entries[0];
            self.repos.add_form.source = Some(source);
            self.repos.add_form.method = Some(method);
            self.repos.add_form.method_index = 0;
        }
    }

    pub(crate) fn try_submit_add_repo(&mut self) -> Option<Action> {
        let source = self.repos.add_form.source.unwrap_or(SourceType::Flatpak);
        let method = self.repos.add_form.method.unwrap_or(RepoMethod::RemoteAdd);

        if self.repos.add_form.url.is_empty() {
            let fields = self.repos.add_form.fields();
            let url_idx = fields
                .iter()
                .position(|f| *f == FormField::Url)
                .unwrap_or(0);
            self.repos.add_form.field = url_idx as u8;
            return None;
        }

        let name = if self.repos.add_form.name.is_empty() && method.needs_name() {
            name_from_url(&self.repos.add_form.url)
        } else {
            self.repos.add_form.name.clone()
        };
        let repo_id = self.repos.add_form.repo_id.clone();
        let check_id = if !repo_id.is_empty() { &repo_id } else { &name };
        let url = &self.repos.add_form.url;
        if self.repos.list.items.iter().any(|r| {
            r.source == source
                && (!check_id.is_empty() && r.id.eq_ignore_ascii_case(check_id)
                    || r.url
                        .as_deref()
                        .is_some_and(|u| u.eq_ignore_ascii_case(url)))
        }) {
            self.set_status(t!("tui.repos.already-exists"));
            self.repos.add_form.active = false;
            self.input_mode = InputMode::Normal;
            return None;
        }

        let url = url.clone();
        let gpgcheck = self.repos.add_form.gpgcheck;
        self.repos.add_form.active = false;
        self.input_mode = InputMode::Normal;
        Some(Action::AddRepo(AddRepoParams {
            method,
            repo_id,
            name,
            url,
            source,
            gpgcheck,
        }))
    }

    pub(crate) fn set_repos(&mut self, repos: Vec<Repository>, view: &mut ViewState) {
        let total = repos.len();
        self.repos.list.items = repos;
        self.repos.list.filter.clear();
        self.repos
            .list
            .sync_selection(&mut view.repos_table, repo_source, repo_matches);
        self.repos.list.loading = false;
        self.repos.list.loaded = true;
        self.status_message = t!("tui.status.repos-count", count = total).to_string();
    }

    pub(crate) fn repos_filtered_indices(&self) -> Vec<usize> {
        self.repos.list.filtered_indices(repo_source, repo_matches)
    }

    fn active_filter_mut(&mut self) -> Option<&mut String> {
        match self.tab {
            Tab::Installed => Some(&mut self.installed.filter),
            Tab::Updates => Some(&mut self.updates.filter),
            Tab::Repos => Some(&mut self.repos.list.filter),
            _ => None,
        }
    }

    fn sync_current_tab_selection(&self, view: &mut ViewState) {
        match self.tab {
            Tab::Search => {
                self.search
                    .results
                    .sync_selection(&mut view.search_table, pkg_source, |_, _| true);
            }
            Tab::Installed => {
                self.installed
                    .sync_selection(&mut view.installed_table, pkg_source, pkg_matches);
            }
            Tab::Updates => {
                self.updates
                    .sync_selection(&mut view.updates_table, update_source, update_matches);
            }
            Tab::Repos => {
                self.repos
                    .list
                    .sync_selection(&mut view.repos_table, repo_source, repo_matches);
            }
            Tab::Settings | Tab::About => {}
        }
    }

    pub(crate) fn current_source_filter(&self) -> Option<SourceType> {
        match self.tab {
            Tab::Search => self.search.results.source_filter,
            Tab::Installed => self.installed.source_filter,
            Tab::Updates => self.updates.source_filter,
            Tab::Repos => self.repos.list.source_filter,
            Tab::Settings | Tab::About => None,
        }
    }

    fn cycle_source_filter(&mut self, view: &mut ViewState) {
        match self.tab {
            Tab::Search => self.search.results.cycle_source_filter(
                &mut view.search_table,
                pkg_source,
                |_, _| true,
            ),
            Tab::Installed => self.installed.cycle_source_filter(
                &mut view.installed_table,
                pkg_source,
                pkg_matches,
            ),
            Tab::Updates => self.updates.cycle_source_filter(
                &mut view.updates_table,
                update_source,
                update_matches,
            ),
            Tab::Repos => self.repos.list.cycle_source_filter(
                &mut view.repos_table,
                repo_source,
                repo_matches,
            ),
            Tab::Settings | Tab::About => {}
        }
    }

    pub(crate) fn needs_installed_load(&self) -> bool {
        self.tab == Tab::Installed && !self.installed.loaded && !self.installed.loading
    }

    pub(crate) fn needs_repos_load(&self) -> bool {
        self.tab == Tab::Repos && !self.repos.list.loaded && !self.repos.list.loading
    }

    pub(crate) fn is_editing_on(&self, tab: Tab) -> bool {
        self.input_mode == InputMode::Editing && self.tab == tab
    }
}
