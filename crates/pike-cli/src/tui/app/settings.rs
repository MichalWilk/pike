use pike_core::config::{Config, SourcesConfig};
use pike_core::package::SourceType;
use rust_i18n::t;

use super::App;
use crate::ipc;
use crate::tui::types::{SettingsRow, ViewState};

fn is_non_activatable(layout: &[SettingsRow], idx: usize) -> bool {
    matches!(
        layout.get(idx),
        Some(SettingsRow::GroupHeader(_) | SettingsRow::Separator | SettingsRow::DaemonStatus)
    )
}

fn build_settings_layout(config: &Config) -> Vec<SettingsRow> {
    let mut rows = Vec::new();
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
            self.cached_settings_layout = Some(build_settings_layout(&self.config));
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
            SettingsRow::GroupHeader(_) | SettingsRow::Separator | SettingsRow::DaemonStatus => {
                false
            }
        }
    }
}
