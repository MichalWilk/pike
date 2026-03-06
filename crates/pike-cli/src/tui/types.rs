use crossterm::event::KeyCode;
use pike_core::package::{RepoMethod, SourceType};
use ratatui::layout::Rect;
use ratatui::widgets::TableState;
use rust_i18n::t;

#[derive(Debug, Clone)]
pub(crate) enum SettingsRow {
    Separator,
    GroupHeader(String),
    SourceToggle(SourceType),
    SourcesReset,
    ArchToggle(SourceType, &'static str),
    ArchReset(SourceType),
    LanguageCycle,
    LogToggle,
    DaemonStatus,
    DaemonInterval,
    NotifyToggle,
}

#[derive(Clone, Copy)]
pub(crate) enum ClickAction {
    SwitchTab(Tab),
    Key(KeyCode),
}

pub(crate) struct ClickTarget {
    pub(crate) rect: Rect,
    pub(crate) action: ClickAction,
}

pub(crate) struct TableClickZone {
    pub(crate) y_start: u16,
    pub(crate) x_start: u16,
    pub(crate) width: u16,
    pub(crate) visible_rows: u16,
    pub(crate) item_count: usize,
}

#[derive(Default)]
pub(crate) struct HitState {
    pub(crate) click_targets: Vec<ClickTarget>,
    pub(crate) table_zone: Option<TableClickZone>,
}

impl HitState {
    pub(crate) fn clear(&mut self) {
        self.click_targets.clear();
        self.table_zone = None;
    }
}

const SPINNER_FRAMES: &[char] = &['|', '/', '-', '\\'];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Search,
    Installed,
    Updates,
    Repos,
    Settings,
    About,
}

impl Tab {
    pub const ALL: [Tab; 6] = [
        Tab::Search,
        Tab::Installed,
        Tab::Updates,
        Tab::Repos,
        Tab::Settings,
        Tab::About,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Tab::Search => "1",
            Tab::Installed => "2",
            Tab::Updates => "3",
            Tab::Repos => "4",
            Tab::Settings => "9",
            Tab::About => "0",
        }
    }

    pub fn title(self) -> String {
        match self {
            Tab::Search => t!("tui.tab.search"),
            Tab::Installed => t!("tui.tab.installed"),
            Tab::Updates => t!("tui.tab.updates"),
            Tab::Repos => t!("tui.tab.repos"),
            Tab::Settings => t!("tui.tab.settings"),
            Tab::About => t!("tui.tab.about"),
        }
        .to_string()
    }

    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|&t| t == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    pub fn prev(self) -> Self {
        let i = Self::ALL.iter().position(|&t| t == self).unwrap_or(0);
        Self::ALL[(i + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Editing,
}

pub(crate) struct ViewState {
    pub(crate) search_table: TableState,
    pub(crate) installed_table: TableState,
    pub(crate) updates_table: TableState,
    pub(crate) repos_table: TableState,
    pub(crate) settings_table: TableState,
    pub(crate) about_table: TableState,
    pub(crate) hover_row: Option<usize>,
    pub(crate) cursor_pointer: bool,
    pub(crate) tick_count: u64,
}

impl ViewState {
    pub(crate) fn new(has_updates: bool) -> Self {
        Self {
            search_table: TableState::default(),
            installed_table: TableState::default(),
            updates_table: if has_updates {
                TableState::default().with_selected(0)
            } else {
                TableState::default()
            },
            repos_table: TableState::default(),
            settings_table: TableState::default().with_selected(1),
            about_table: TableState::default().with_selected(0),
            hover_row: None,
            cursor_pointer: false,
            tick_count: 0,
        }
    }

    pub(crate) fn tick(&mut self) {
        self.tick_count = self.tick_count.wrapping_add(1);
    }

    pub(crate) fn spinner_char(&self) -> char {
        SPINNER_FRAMES[(self.tick_count as usize) % SPINNER_FRAMES.len()]
    }

    pub(crate) fn table_for(&mut self, tab: Tab) -> &mut TableState {
        match tab {
            Tab::Search => &mut self.search_table,
            Tab::Installed => &mut self.installed_table,
            Tab::Updates => &mut self.updates_table,
            Tab::Repos => &mut self.repos_table,
            Tab::Settings => &mut self.settings_table,
            Tab::About => &mut self.about_table,
        }
    }

    pub(crate) fn table_offset(&self, tab: Tab) -> usize {
        match tab {
            Tab::Search => self.search_table.offset(),
            Tab::Installed => self.installed_table.offset(),
            Tab::Updates => self.updates_table.offset(),
            Tab::Repos => self.repos_table.offset(),
            Tab::Settings => self.settings_table.offset(),
            Tab::About => 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FormField {
    RepoId,
    Name,
    Url,
    GpgCheck,
}

pub(crate) struct AddRepoParams {
    pub(crate) method: RepoMethod,
    pub(crate) repo_id: String,
    pub(crate) name: String,
    pub(crate) url: String,
    pub(crate) source: SourceType,
    pub(crate) gpgcheck: bool,
}

pub enum Action {
    Quit,
    SearchSubmit(String),
    InstallPackage(String, Option<SourceType>),
    RemovePackage(String, Option<SourceType>),
    UpdatePackage(String, SourceType),
    UpdateAll(Vec<(String, SourceType)>),
    Autoremove,
    RefreshUpdates,
    RefreshInstalled,
    RefreshRepos,
    ToggleRepo(String, bool, SourceType),
    AddRepo(AddRepoParams),
    DeleteRepo(String, SourceType),
    OpenUrl(String),
    SaveSettings,
}
