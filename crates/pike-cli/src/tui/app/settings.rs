use crossterm::event::{KeyCode, KeyEvent};
use pike_core::config::{Config, DEFAULT_ACCENT_COLOR, SourcesConfig, parse_hex_color};
use pike_core::package::SourceType;
use rust_i18n::t;

use super::App;
use crate::ipc;
use crate::tui::types::{Action, InputMode, SettingsRow, ViewState};
use crate::tui::ui::{DEFAULT_ACCENT_RGB, set_accent};

const ACCENT_PRESETS: &[(&str, &str)] = &[
    (DEFAULT_ACCENT_COLOR, "tui.settings.accent-orange"),
    ("#3b8eea", "tui.settings.accent-blue"),
    ("#00b894", "tui.settings.accent-green"),
    ("#bf5af2", "tui.settings.accent-purple"),
    ("#e5484d", "tui.settings.accent-red"),
];

fn is_non_activatable(layout: &[SettingsRow], idx: usize) -> bool {
    matches!(
        layout.get(idx),
        Some(SettingsRow::GroupHeader(_) | SettingsRow::Separator | SettingsRow::DaemonStatus)
    )
}

fn next_keep_kernels(current: usize) -> usize {
    if current >= 5 { 1 } else { current + 1 }
}

fn build_settings_layout(config: &Config, accent_custom: bool) -> Vec<SettingsRow> {
    let mut rows = Vec::new();
    rows.push(SettingsRow::GroupHeader(
        t!("tui.settings.display").to_string(),
    ));
    rows.push(SettingsRow::LanguageCycle);
    rows.push(SettingsRow::ConfirmToggle);
    rows.push(SettingsRow::AccentCycle);
    if accent_custom {
        rows.push(SettingsRow::AccentHex);
    }
    rows.push(SettingsRow::Separator);
    rows.push(SettingsRow::GroupHeader(
        t!("tui.settings.sources").to_string(),
    ));
    for &st in SourceType::ALL {
        rows.push(SettingsRow::SourceToggle(st));
    }
    rows.push(SettingsRow::SourcesReset);
    for &st in SourceType::ALL {
        if !config.sources.enabled(st) || !st.has_arch_filter() {
            continue;
        }
        rows.push(SettingsRow::Separator);
        rows.push(SettingsRow::GroupHeader(
            t!("tui.settings.architectures", source = st.display_name()).to_string(),
        ));
        for &arch in st.known_arches() {
            rows.push(SettingsRow::ArchToggle(st, arch));
        }
        rows.push(SettingsRow::ArchReset(st));
    }
    rows.push(SettingsRow::Separator);
    rows.push(SettingsRow::GroupHeader(
        t!("tui.settings.cleanup").to_string(),
    ));
    rows.push(SettingsRow::KeepKernels);
    rows.push(SettingsRow::Separator);
    rows.push(SettingsRow::GroupHeader(
        t!("tui.settings.logging").to_string(),
    ));
    rows.push(SettingsRow::LogToggle);
    rows.push(SettingsRow::Separator);
    rows.push(SettingsRow::GroupHeader(
        t!("tui.settings.daemon").to_string(),
    ));
    rows.push(SettingsRow::DaemonStatus);
    rows.push(SettingsRow::DaemonInterval);
    rows.push(SettingsRow::NotifyToggle);
    rows
}

impl App {
    pub(crate) fn ensure_settings_cache(&mut self) {
        if self.cached_settings_layout.is_none() {
            self.cached_settings_layout =
                Some(build_settings_layout(&self.config, self.is_custom_accent()));
        }
    }

    pub(crate) fn refresh_daemon_status(&mut self) {
        self.daemon_running = ipc::is_daemon_running();
    }

    pub(crate) fn settings_layout(&self) -> &[SettingsRow] {
        self.cached_settings_layout.as_deref().unwrap_or(&[])
    }

    pub(crate) fn invalidate_settings_cache(&mut self) {
        self.cached_settings_layout = None;
    }

    pub(crate) fn settings_count(&self) -> usize {
        self.settings_layout().len()
    }

    pub(crate) fn settings_skip_groups(&self, direction: i32, view: &mut ViewState) {
        let layout = self.settings_layout();
        let max = layout.len();
        if max == 0 {
            return;
        }
        if let Some(mut idx) = view.settings_table.selected()
            && is_non_activatable(layout, idx)
        {
            let start = idx;
            loop {
                idx = if direction >= 0 {
                    (idx + 1).min(max - 1)
                } else {
                    idx.saturating_sub(1)
                };
                if !is_non_activatable(layout, idx) || idx == start {
                    break;
                }
                if (direction >= 0 && idx == max - 1) || (direction < 0 && idx == 0) {
                    break;
                }
            }
            view.settings_table.select(Some(idx));
        }
    }

    fn accent_preset_index(&self) -> Option<usize> {
        if self.accent_custom {
            return None;
        }
        ACCENT_PRESETS
            .iter()
            .position(|(h, _)| h.eq_ignore_ascii_case(&self.config.display.accent_color))
    }

    pub(crate) fn accent_preset_key(&self) -> Option<&'static str> {
        self.accent_preset_index()
            .and_then(|i| ACCENT_PRESETS.get(i))
            .map(|(_, key)| *key)
    }

    pub(crate) fn is_custom_accent(&self) -> bool {
        self.accent_preset_key().is_none()
    }

    pub(crate) fn handle_settings_input_key(&mut self, key: KeyEvent) -> Vec<Action> {
        let Some(input) = self.settings_input.as_mut() else {
            return vec![];
        };
        match key.code {
            KeyCode::Char(c) if input.len() < 7 && c.is_ascii_hexdigit() => {
                input.push(c);
                vec![]
            }
            KeyCode::Backspace => {
                if input.len() > 1 {
                    input.pop();
                }
                vec![]
            }
            KeyCode::Esc => {
                self.settings_input = None;
                self.input_mode = InputMode::Normal;
                self.status_message.clear();
                vec![]
            }
            KeyCode::Enter => match parse_hex_color(input) {
                Some(rgb) => {
                    let hex = input.to_ascii_lowercase();
                    self.settings_input = None;
                    self.input_mode = InputMode::Normal;
                    self.status_message.clear();
                    self.accent_custom = true;
                    self.apply_accent(&hex, rgb);
                    vec![Action::SaveSettings]
                }
                None => {
                    self.set_status(t!("tui.status.invalid-color"));
                    vec![]
                }
            },
            _ => vec![],
        }
    }

    pub(crate) fn init_accent(&mut self) {
        let parsed = parse_hex_color(&self.config.display.accent_color);
        if parsed.is_none() {
            tracing::warn!(
                "invalid accent_color {:?}, using default",
                self.config.display.accent_color
            );
            self.config.display.accent_color = DEFAULT_ACCENT_COLOR.to_string();
            self.accent_reset = true;
            self.invalidate_settings_cache();
        }
        set_accent(parsed.unwrap_or(DEFAULT_ACCENT_RGB));
    }

    /// Clears the accent reset note once the config file has been rewritten.
    pub(crate) fn settings_saved(&mut self) {
        self.accent_reset = false;
    }

    fn apply_accent(&mut self, hex: &str, rgb: (u8, u8, u8)) {
        self.config.display.accent_color = hex.to_string();
        self.accent_reset = false;
        set_accent(rgb);
        self.invalidate_settings_cache();
    }

    pub(crate) fn activate_selected_setting(&mut self, view: &mut ViewState) -> bool {
        let idx = view.settings_table.selected().unwrap_or(1);
        let row = match self.settings_layout().get(idx).cloned() {
            Some(r) => r,
            None => return false,
        };
        match row {
            SettingsRow::SourceToggle(st) => {
                let was_enabled = self.config.sources.enabled(st);
                if !was_enabled && !st.is_available() {
                    self.set_status(t!(
                        "tui.status.source-not-installed",
                        source = st.display_name()
                    ));
                    return false;
                }
                self.config.sources.set_enabled(st, !was_enabled);
                self.invalidate_settings_cache();
                if was_enabled {
                    self.ensure_settings_cache();
                    let new_count = self.settings_count();
                    let sel = view
                        .settings_table
                        .selected()
                        .unwrap_or(1)
                        .min(new_count.saturating_sub(1));
                    view.settings_table.select(Some(sel));
                } else {
                    self.config
                        .display
                        .architectures
                        .set_arches(st, st.default_arches());
                }
                true
            }
            SettingsRow::SourcesReset => {
                self.config.sources = SourcesConfig::detect();
                for &st in SourceType::ALL {
                    self.config
                        .display
                        .architectures
                        .set_arches(st, st.default_arches());
                }
                self.invalidate_settings_cache();
                true
            }
            SettingsRow::ArchToggle(st, arch) => {
                let mut arches: Vec<String> = self.config.display.architectures.arches(st).to_vec();
                if let Some(pos) = arches.iter().position(|a| a == arch) {
                    arches.remove(pos);
                } else {
                    arches.push(arch.to_string());
                }
                self.config.display.architectures.set_arches(st, arches);
                self.invalidate_settings_cache();
                true
            }
            SettingsRow::ArchReset(st) => {
                self.config
                    .display
                    .architectures
                    .set_arches(st, st.default_arches());
                self.invalidate_settings_cache();
                true
            }
            SettingsRow::LanguageCycle => {
                const LANGUAGES: &[&str] = &["auto", "en", "pl"];
                let current = &self.config.display.language;
                let idx = LANGUAGES.iter().position(|&l| l == current).unwrap_or(0);
                let next = LANGUAGES[(idx + 1) % LANGUAGES.len()];
                self.config.display.language = next.to_string();
                let locale = if next == "auto" {
                    sys_locale::get_locale()
                        .unwrap_or_else(|| "en".into())
                        .split(['-', '_'])
                        .next()
                        .unwrap_or("en")
                        .to_string()
                } else {
                    next.to_string()
                };
                rust_i18n::set_locale(&locale);
                self.invalidate_settings_cache();
                true
            }
            SettingsRow::LogToggle => {
                self.config.logging.file = !self.config.logging.file;
                self.invalidate_settings_cache();
                true
            }
            SettingsRow::DaemonInterval => {
                const PRESETS: &[u64] = &[300, 600, 900, 1800, 3600, 7200, 14400, 28800, 86400];
                let current = self.config.daemon.interval;
                let next = PRESETS
                    .iter()
                    .find(|&&v| v > current)
                    .copied()
                    .unwrap_or(PRESETS[0]);
                self.config.daemon.interval = next;
                self.invalidate_settings_cache();
                true
            }
            SettingsRow::NotifyToggle => {
                self.config.daemon.notify = !self.config.daemon.notify;
                self.invalidate_settings_cache();
                true
            }
            SettingsRow::ConfirmToggle => {
                self.config.display.confirm_actions = !self.config.display.confirm_actions;
                self.invalidate_settings_cache();
                true
            }
            SettingsRow::AccentCycle => {
                let next = self
                    .accent_preset_index()
                    .map_or(Some(0), |i| (i + 1 < ACCENT_PRESETS.len()).then_some(i + 1))
                    .and_then(|i| ACCENT_PRESETS.get(i));
                match next {
                    Some((hex, _)) => {
                        if let Some(rgb) = parse_hex_color(hex) {
                            self.accent_custom = false;
                            self.apply_accent(hex, rgb);
                        }
                        true
                    }
                    None => {
                        self.accent_custom = true;
                        self.invalidate_settings_cache();
                        false
                    }
                }
            }
            SettingsRow::AccentHex => {
                self.settings_input = Some("#".to_string());
                self.input_mode = InputMode::Editing;
                false
            }
            SettingsRow::KeepKernels => {
                self.config.cleanup.keep_kernels =
                    next_keep_kernels(self.config.cleanup.keep_kernels());
                self.cleanup.loaded = false;
                self.invalidate_settings_cache();
                true
            }
            SettingsRow::GroupHeader(_) | SettingsRow::Separator | SettingsRow::DaemonStatus => {
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings_app() -> (App, ViewState) {
        let mut app = App::new(Config::default(), Vec::new(), vec![SourceType::Dnf]);
        app.tab = crate::tui::types::Tab::Settings;
        app.ensure_settings_cache();
        let mut view = ViewState::new(false);
        let idx = app
            .settings_layout()
            .iter()
            .position(|r| matches!(r, SettingsRow::AccentCycle));
        view.settings_table.select(idx);
        (app, view)
    }

    fn select_row(app: &mut App, view: &mut ViewState, is_row: fn(&SettingsRow) -> bool) {
        app.ensure_settings_cache();
        let idx = app.settings_layout().iter().position(is_row);
        view.settings_table.select(idx);
    }

    fn has_hex_row(app: &mut App) -> bool {
        app.ensure_settings_cache();
        app.settings_layout()
            .iter()
            .any(|r| matches!(r, SettingsRow::AccentHex))
    }

    fn enter_custom_hex(app: &mut App, view: &mut ViewState) {
        app.config.display.accent_color = "#e5484d".into();
        key(app, view, KeyCode::Char('e'));
        select_row(app, view, |r| matches!(r, SettingsRow::AccentHex));
        key(app, view, KeyCode::Char('e'));
    }

    fn key(app: &mut App, view: &mut ViewState, code: KeyCode) -> Vec<Action> {
        app.handle_key(KeyEvent::from(code), view)
    }

    fn type_str(app: &mut App, view: &mut ViewState, s: &str) {
        for c in s.chars() {
            key(app, view, KeyCode::Char(c));
        }
    }

    #[test]
    fn test_accent_cycle_custom_then_first_preset() {
        let (mut app, mut view) = settings_app();
        app.config.display.accent_color = "#e5484d".into();
        assert!(key(&mut app, &mut view, KeyCode::Char('e')).is_empty());
        assert_eq!(app.input_mode, InputMode::Normal);
        assert!(app.is_custom_accent());
        assert!(has_hex_row(&mut app));
        assert_eq!(app.config.display.accent_color, "#e5484d");
        let actions = key(&mut app, &mut view, KeyCode::Char('e'));
        assert!(matches!(actions.as_slice(), [Action::SaveSettings]));
        assert_eq!(app.config.display.accent_color, "#fe8019");
        assert!(!has_hex_row(&mut app));
    }

    #[test]
    fn test_custom_hex_from_config_shows_hex_row() {
        let (mut app, mut view) = settings_app();
        app.config.display.accent_color = "#123456".into();
        app.invalidate_settings_cache();
        app.ensure_settings_cache();
        assert!(has_hex_row(&mut app));
        key(&mut app, &mut view, KeyCode::Char('e'));
        assert_eq!(app.config.display.accent_color, "#fe8019");
        assert!(!has_hex_row(&mut app));
    }

    #[test]
    fn test_custom_hex_valid_saves() {
        let (mut app, mut view) = settings_app();
        enter_custom_hex(&mut app, &mut view);
        assert_eq!(app.input_mode, InputMode::Editing);
        assert_eq!(app.settings_input.as_deref(), Some("#"));
        type_str(&mut app, &mut view, "E5zC76xB");
        assert_eq!(app.settings_input.as_deref(), Some("#E5C76B"));
        let actions = key(&mut app, &mut view, KeyCode::Enter);
        assert!(matches!(actions.as_slice(), [Action::SaveSettings]));
        assert_eq!(app.config.display.accent_color, "#e5c76b");
        assert_eq!(app.input_mode, InputMode::Normal);
        assert!(app.settings_input.is_none());
        assert!(has_hex_row(&mut app));
    }

    #[test]
    fn test_custom_hex_invalid_keeps_editing() {
        let (mut app, mut view) = settings_app();
        enter_custom_hex(&mut app, &mut view);
        type_str(&mut app, &mut view, "ff453");
        assert!(key(&mut app, &mut view, KeyCode::Enter).is_empty());
        assert_eq!(app.input_mode, InputMode::Editing);
        assert_eq!(app.settings_input.as_deref(), Some("#ff453"));
        assert_eq!(app.config.display.accent_color, "#e5484d");
        assert!(!app.status_message.is_empty());
    }

    #[test]
    fn test_custom_hex_esc_cancels() {
        let (mut app, mut view) = settings_app();
        enter_custom_hex(&mut app, &mut view);
        type_str(&mut app, &mut view, "0");
        assert!(key(&mut app, &mut view, KeyCode::Esc).is_empty());
        assert_eq!(app.input_mode, InputMode::Normal);
        assert!(app.settings_input.is_none());
        assert_eq!(app.config.display.accent_color, "#e5484d");
        assert!(has_hex_row(&mut app));
    }

    #[test]
    fn test_custom_hex_keeps_hash_and_caps_length() {
        let (mut app, mut view) = settings_app();
        enter_custom_hex(&mut app, &mut view);
        for _ in 0..3 {
            key(&mut app, &mut view, KeyCode::Backspace);
        }
        type_str(&mut app, &mut view, "a#bcdefg12");
        assert_eq!(app.settings_input.as_deref(), Some("#abcdef"));
    }

    #[test]
    fn test_custom_hex_preset_value_keeps_custom_row() {
        let (mut app, mut view) = settings_app();
        app.config.display.accent_color = "#123456".into();
        app.invalidate_settings_cache();
        select_row(&mut app, &mut view, |r| matches!(r, SettingsRow::AccentHex));
        let before = view.settings_table.selected();
        key(&mut app, &mut view, KeyCode::Char('e'));
        type_str(&mut app, &mut view, "3b8eea");
        let actions = key(&mut app, &mut view, KeyCode::Enter);
        assert!(matches!(actions.as_slice(), [Action::SaveSettings]));
        assert_eq!(app.config.display.accent_color, "#3b8eea");
        assert!(has_hex_row(&mut app));
        assert_eq!(view.settings_table.selected(), before);
        assert!(matches!(
            before.and_then(|i| app.settings_layout().get(i)),
            Some(SettingsRow::AccentHex)
        ));
    }

    #[test]
    fn test_switch_tab_closes_input_without_saving() {
        let (mut app, mut view) = settings_app();
        enter_custom_hex(&mut app, &mut view);
        type_str(&mut app, &mut view, "123456");
        app.switch_tab(crate::tui::types::Tab::Search);
        assert!(app.settings_input.is_none());
        assert_eq!(app.input_mode, InputMode::Normal);
        assert_eq!(app.config.display.accent_color, "#e5484d");
    }

    #[test]
    fn test_accent_full_key_cycle() {
        let (mut app, mut view) = settings_app();
        assert_eq!(app.config.display.accent_color, DEFAULT_ACCENT_COLOR);
        let steps = [
            "#3b8eea", "#00b894", "#bf5af2", "#e5484d", "#e5484d", "#fe8019",
        ];
        for (i, hex) in steps.iter().enumerate() {
            app.ensure_settings_cache();
            key(&mut app, &mut view, KeyCode::Char('e'));
            assert_eq!(app.config.display.accent_color, *hex);
            assert_eq!(app.is_custom_accent(), i == 4);
        }
    }

    #[test]
    fn test_init_accent_resets_invalid_color() {
        let (mut app, mut view) = settings_app();
        app.config.display.accent_color = "orange".into();
        app.init_accent();
        assert_eq!(app.config.display.accent_color, DEFAULT_ACCENT_COLOR);
        assert!(!app.is_custom_accent());
        assert!(app.accent_reset);
        select_row(&mut app, &mut view, |r| {
            matches!(r, SettingsRow::AccentCycle)
        });
        key(&mut app, &mut view, KeyCode::Char('e'));
        assert!(!app.accent_reset);
        app.accent_reset = true;
        app.settings_saved();
        assert!(!app.accent_reset);
    }

    #[test]
    fn test_init_accent_valid_color_keeps_flag_clear() {
        let (mut app, _) = settings_app();
        app.config.display.accent_color = "#123456".into();
        app.init_accent();
        assert_eq!(app.config.display.accent_color, "#123456");
        assert!(!app.accent_reset);
    }
}
