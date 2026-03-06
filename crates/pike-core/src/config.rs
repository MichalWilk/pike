use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::PikeError;
use crate::package::SourceType;

#[derive(Debug, Clone)]
pub struct SourcesConfig {
    map: BTreeMap<SourceType, bool>,
}

impl SourcesConfig {
    pub fn enabled(&self, st: SourceType) -> bool {
        self.map.get(&st).copied().unwrap_or(true)
    }

    pub fn set_enabled(&mut self, st: SourceType, value: bool) {
        self.map.insert(st, value);
    }

    pub fn iter(&self) -> impl Iterator<Item = (SourceType, bool)> + '_ {
        self.map.iter().map(|(&k, &v)| (k, v))
    }

    pub fn detect() -> Self {
        let map = SourceType::ALL
            .iter()
            .map(|&st| (st, st.is_available()))
            .collect();
        Self { map }
    }
}

impl Default for SourcesConfig {
    fn default() -> Self {
        let map = SourceType::ALL.iter().map(|&st| (st, true)).collect();
        Self { map }
    }
}

impl Serialize for SourcesConfig {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let string_map: BTreeMap<String, bool> = self
            .map
            .iter()
            .map(|(st, &v)| (st.display_name().to_string(), v))
            .collect();
        string_map.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SourcesConfig {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let string_map = BTreeMap::<String, bool>::deserialize(deserializer)?;
        let mut map = BTreeMap::new();
        for &st in SourceType::ALL {
            let enabled = string_map.get(st.display_name()).copied().unwrap_or(true);
            map.insert(st, enabled);
        }
        Ok(Self { map })
    }
}

#[derive(Debug, Clone)]
pub struct ArchConfig {
    map: BTreeMap<SourceType, Vec<String>>,
}

impl Default for ArchConfig {
    fn default() -> Self {
        let map = SourceType::ALL
            .iter()
            .map(|&st| (st, st.default_arches()))
            .collect();
        Self { map }
    }
}

impl ArchConfig {
    pub fn arches(&self, st: SourceType) -> &[String] {
        self.map.get(&st).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub fn set_arches(&mut self, st: SourceType, arches: Vec<String>) {
        self.map.insert(st, arches);
    }

    pub fn arch_allowed(&self, arch: &str, source: SourceType) -> bool {
        self.arches(source).iter().any(|a| a == arch)
    }
}

impl Serialize for ArchConfig {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let string_map: BTreeMap<String, Vec<String>> = self
            .map
            .iter()
            .map(|(st, v)| (st.display_name().to_string(), v.clone()))
            .collect();
        string_map.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ArchConfig {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let table = BTreeMap::<String, Vec<String>>::deserialize(deserializer)?;
        let mut map = BTreeMap::new();
        for &st in SourceType::ALL {
            let arches = table
                .get(st.display_name())
                .cloned()
                .unwrap_or_else(|| st.default_arches());
            map.insert(st, arches);
        }
        Ok(Self { map })
    }
}

fn default_language() -> String {
    "auto".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayConfig {
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default)]
    pub architectures: ArchConfig,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            language: default_language(),
            architectures: ArchConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    pub file: bool,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self { file: true }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum IconStyle {
    #[default]
    Nerd,
    Unicode,
}

impl IconStyle {
    pub fn detect() -> Self {
        let ok = std::process::Command::new("fc-list")
            .args([":", "family"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output()
            .is_ok_and(|o| {
                o.status.success()
                    && String::from_utf8_lossy(&o.stdout)
                        .lines()
                        .any(|l| l.to_lowercase().contains("nerd"))
            });
        if ok { Self::Nerd } else { Self::Unicode }
    }
}

fn default_daemon_interval() -> u64 {
    600
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    #[serde(default = "default_daemon_interval")]
    pub interval: u64,
    #[serde(default = "default_true")]
    pub notify: bool,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            interval: default_daemon_interval(),
            notify: true,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    pub sources: SourcesConfig,
    #[serde(default)]
    pub display: DisplayConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
    #[serde(default)]
    pub daemon: DaemonConfig,
}

impl Config {
    pub fn path() -> Result<PathBuf, PikeError> {
        let config_dir = dirs::config_dir()
            .ok_or_else(|| PikeError::Config("could not determine config directory".into()))?;
        Ok(config_dir.join("pike").join("config.toml"))
    }

    pub fn load() -> Result<Self, PikeError> {
        let path = Self::path()?;

        if !path.exists() {
            let config = Config {
                sources: SourcesConfig::detect(),
                display: DisplayConfig::default(),
                logging: LoggingConfig::default(),
                daemon: DaemonConfig::default(),
            };
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, config.to_toml_commented())?;
            tracing::debug!("created default config at {}", path.display());
            return Ok(config);
        }

        let contents = std::fs::read_to_string(&path)?;
        let config: Config =
            toml::from_str(&contents).map_err(|e| PikeError::Config(e.to_string()))?;
        tracing::debug!("loaded config from {}", path.display());
        Ok(config)
    }

    pub fn to_toml_commented(&self) -> String {
        let mut out = String::new();
        out.push_str("[sources]\n");
        for (st, enabled) in self.sources.iter() {
            out.push_str(&format!("{} = {enabled}\n", st.display_name()));
        }

        out.push_str(&format!(
            "\n[display]\n\
             # Language: \"auto\" (detect from system), \"en\", or \"pl\"\n\
             language = \"{}\"\n",
            self.display.language
        ));

        out.push_str(
            "\n# Architecture filters per source.\n\
             # Only packages matching these architectures are shown in search results.\n\
             # Defaults are auto-detected from host architecture.\n\
             [display.architectures]\n",
        );
        for &st in SourceType::ALL {
            if !st.has_arch_filter() {
                continue;
            }
            out.push_str(&format!("# Available: {}\n", st.known_arches().join(", ")));
            let arches = self.display.architectures.arches(st);
            let quoted: Vec<String> = arches.iter().map(|a| format!("\"{a}\"")).collect();
            out.push_str(&format!(
                "{} = [{}]\n",
                st.display_name(),
                quoted.join(", ")
            ));
        }

        out.push_str(&format!("\n[logging]\nfile = {}\n", self.logging.file));

        out.push_str(&format!(
            "\n[daemon]\n# Check interval in seconds (default: 600 = 10 minutes)\n\
             interval = {}\n\
             # Show desktop notifications when updates are found\n\
             notify = {}\n",
            self.daemon.interval, self.daemon.notify
        ));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert!(config.sources.enabled(SourceType::Dnf));
        assert!(config.sources.enabled(SourceType::Flatpak));
        let dnf_arches = config.display.architectures.arches(SourceType::Dnf);
        assert!(dnf_arches.iter().any(|a| a == std::env::consts::ARCH));
        assert!(dnf_arches.iter().any(|a| a == "noarch"));
        let flatpak_arches = config.display.architectures.arches(SourceType::Flatpak);
        assert!(flatpak_arches.is_empty());
    }

    #[test]
    fn test_config_roundtrip() {
        let config = Config::default();
        let toml_str = config.to_toml_commented();
        let parsed: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(
            parsed.sources.enabled(SourceType::Dnf),
            config.sources.enabled(SourceType::Dnf)
        );
        assert_eq!(
            parsed.sources.enabled(SourceType::Flatpak),
            config.sources.enabled(SourceType::Flatpak)
        );
        for &st in SourceType::ALL {
            assert_eq!(
                parsed.display.architectures.arches(st),
                config.display.architectures.arches(st)
            );
        }
        assert_eq!(parsed.logging.file, config.logging.file);
        assert_eq!(parsed.display.language, config.display.language);
    }

    #[test]
    fn test_arch_allowed_per_source() {
        let mut arch = ArchConfig::default();
        arch.set_arches(SourceType::Dnf, vec!["x86_64".into(), "noarch".into()]);
        assert!(arch.arch_allowed("x86_64", SourceType::Dnf));
        assert!(arch.arch_allowed("noarch", SourceType::Dnf));
        assert!(!arch.arch_allowed("aarch64", SourceType::Dnf));
        assert!(!arch.arch_allowed("x86_64", SourceType::Flatpak));
    }

    #[test]
    fn test_config_without_display_section() {
        let toml_str = "[sources]\ndnf = true\nflatpak = true\n";
        let parsed: Config = toml::from_str(toml_str).unwrap();
        let dnf_arches = parsed.display.architectures.arches(SourceType::Dnf);
        assert!(dnf_arches.iter().any(|a| a == std::env::consts::ARCH));
    }
}
