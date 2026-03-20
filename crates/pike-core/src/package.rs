use std::collections::BTreeMap;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub source: SourceType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy, Hash, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum SourceType {
    Dnf,
    Flatpak,
    Apt,
}

impl SourceType {
    pub const ALL: &[SourceType] = &[SourceType::Dnf, SourceType::Flatpak, SourceType::Apt];

    pub fn binary_name(self) -> &'static str {
        match self {
            Self::Dnf => "dnf5",
            Self::Flatpak => "flatpak",
            Self::Apt => "apt-get",
        }
    }

    pub fn is_available(self) -> bool {
        std::process::Command::new("which")
            .arg(self.binary_name())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Dnf => "dnf",
            Self::Flatpak => "flatpak",
            Self::Apt => "apt",
        }
    }

    pub fn has_arch_filter(self) -> bool {
        match self {
            Self::Dnf | Self::Apt => true,
            Self::Flatpak => false,
        }
    }

    pub fn known_arches(self) -> &'static [&'static str] {
        match self {
            Self::Dnf => &[
                "x86_64", "aarch64", "i686", "noarch", "armv7hl", "ppc64le", "s390x",
            ],
            Self::Apt => &["amd64", "arm64", "armhf", "i386", "all"],
            Self::Flatpak => &[],
        }
    }

    pub fn default_arches(self) -> Vec<String> {
        let host = std::env::consts::ARCH;
        match self {
            Self::Dnf => {
                let mapped = match host {
                    "x86" => "i686",
                    "arm" => "armv7hl",
                    other => other,
                };
                vec![mapped.to_string(), "noarch".to_string()]
            }
            Self::Apt => {
                let mapped = match host {
                    "x86_64" => "amd64",
                    "aarch64" => "arm64",
                    "arm" => "armhf",
                    "x86" => "i386",
                    other => other,
                };
                vec![mapped.to_string(), "all".to_string()]
            }
            Self::Flatpak => Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoMethod {
    RepoFile,
    Copr,
    BaseUrl,
    RpmPackage,
    RemoteAdd,
    Ppa,
}

impl RepoMethod {
    pub fn methods_for(source: SourceType) -> &'static [RepoMethod] {
        match source {
            SourceType::Dnf => &[
                RepoMethod::RepoFile,
                RepoMethod::Copr,
                RepoMethod::BaseUrl,
                RepoMethod::RpmPackage,
            ],
            SourceType::Apt => &[RepoMethod::Ppa, RepoMethod::BaseUrl],
            SourceType::Flatpak => &[RepoMethod::RemoteAdd],
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::RepoFile => ".repo file URL",
            Self::Copr => "COPR (owner/project)",
            Self::BaseUrl => "Base URL",
            Self::RpmPackage => "RPM package URL",
            Self::RemoteAdd => "Flatpak remote",
            Self::Ppa => "PPA (user/ppa-name)",
        }
    }

    pub fn has_repo_id(self) -> bool {
        matches!(self, Self::RepoFile | Self::BaseUrl)
    }

    pub fn has_display_name(self) -> bool {
        matches!(self, Self::RepoFile | Self::BaseUrl)
    }

    pub fn needs_name(self) -> bool {
        matches!(self, Self::RemoteAdd)
    }

    pub fn has_gpgcheck(self) -> bool {
        matches!(self, Self::RepoFile | Self::BaseUrl)
    }

    pub fn url_label(self) -> &'static str {
        match self {
            Self::Copr => "owner/project",
            Self::Ppa => "user/ppa-name",
            _ => "URL",
        }
    }
}

impl std::fmt::Display for RepoMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.display_name())
    }
}

impl std::fmt::Display for SourceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.display_name())
    }
}

impl FromStr for SourceType {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Self::ALL
            .iter()
            .find(|st| st.display_name() == s)
            .copied()
            .ok_or_else(|| {
                let names: Vec<_> = Self::ALL.iter().map(|st| st.display_name()).collect();
                format!(
                    "unknown source '{s}', expected one of: {}",
                    names.join(", ")
                )
            })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repository {
    pub id: String,
    pub name: String,
    pub source: SourceType,
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageUpdate {
    pub name: String,
    pub source: SourceType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
    pub installed_version: String,
    pub available_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusSummary {
    pub total: usize,
    pub counts: BTreeMap<SourceType, usize>,
    pub updates: Vec<PackageUpdate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<String>,
}

impl StatusSummary {
    pub fn from_updates(updates: Vec<PackageUpdate>) -> Self {
        let mut counts = BTreeMap::new();
        for u in &updates {
            *counts.entry(u.source).or_insert(0) += 1;
        }
        Self {
            total: updates.len(),
            counts,
            updates,
            checked_at: None,
        }
    }

    pub fn from_fresh_check(updates: Vec<PackageUpdate>) -> Self {
        let mut s = Self::from_updates(updates);
        s.checked_at = Some(chrono::Utc::now().to_rfc3339());
        s
    }

    pub fn count(&self, st: SourceType) -> usize {
        self.counts.get(&st).copied().unwrap_or(0)
    }
}
