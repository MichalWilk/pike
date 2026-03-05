use async_trait::async_trait;

use crate::package::{Package, PackageUpdate, RepoMethod, Repository, SourceType};
use crate::source::{
    PackageSource, Result, parse_installed_versions, run_captured, run_interactive,
};

#[derive(Default)]
pub struct FlatpakSource;

#[async_trait]
impl PackageSource for FlatpakSource {
    fn name(&self) -> &str {
        "flatpak"
    }

    fn source_type(&self) -> SourceType {
        SourceType::Flatpak
    }

    async fn search(&self, query: &str) -> Result<Vec<Package>> {
        let output = run_captured(
            "flatpak",
            &[
                "search",
                query,
                "--columns=name,description,application,version,remotes",
            ],
        )
        .await?;
        Ok(parse_search_output(&output))
    }

    async fn install(&self, package: &str) -> Result<()> {
        let app_id = self.resolve_app_id(package).await?;
        run_interactive("flatpak", &["install", "-y", &app_id]).await
    }

    async fn remove(&self, package: &str, purge: bool) -> Result<()> {
        let app_id = self.resolve_app_id(package).await?;
        if purge {
            run_interactive("flatpak", &["uninstall", "-y", "--delete-data", &app_id]).await
        } else {
            run_interactive("flatpak", &["uninstall", "-y", &app_id]).await
        }
    }

    async fn autoremove(&self) -> Result<()> {
        run_interactive("flatpak", &["uninstall", "--unused", "-y"]).await
    }

    async fn check_updates(&self) -> Result<Vec<PackageUpdate>> {
        let output = run_captured(
            "flatpak",
            &[
                "remote-ls",
                "--updates",
                "--columns=name,application,version,arch",
            ],
        )
        .await?;
        let mut updates = parse_updates_output(&output);

        let installed_output = run_captured(
            "flatpak",
            &["list", "--app", "--columns=application,version"],
        )
        .await?;
        let installed_versions = parse_installed_versions(&installed_output, '\t');
        for u in &mut updates {
            if let Some(ver) = installed_versions.get(u.name.as_str()) {
                u.installed_version.clone_from(ver);
            }
        }

        Ok(updates)
    }

    async fn update(&self, package: &str) -> Result<()> {
        let app_id = self.resolve_app_id(package).await?;
        run_interactive("flatpak", &["update", "-y", &app_id]).await
    }

    async fn update_all(&self) -> Result<()> {
        run_interactive("flatpak", &["update", "-y"]).await
    }

    async fn list_installed(&self) -> Result<Vec<Package>> {
        let output = run_captured(
            "flatpak",
            &[
                "list",
                "--app",
                "--columns=name,application,version,arch,description",
            ],
        )
        .await?;
        Ok(parse_list_installed_output(&output))
    }

    async fn list_repos(&self) -> Result<Vec<Repository>> {
        let output = run_captured(
            "flatpak",
            &[
                "remotes",
                "--show-disabled",
                "--columns=name,title,url,options",
            ],
        )
        .await?;
        Ok(parse_remotes_output(&output))
    }

    async fn set_repo_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        let flag = if enabled { "--enable" } else { "--disable" };
        run_interactive("flatpak", &["remote-modify", flag, id]).await
    }

    async fn add_repo(
        &self,
        method: RepoMethod,
        _repo_id: &str,
        name: &str,
        url: &str,
        _gpgcheck: bool,
    ) -> Result<()> {
        match method {
            RepoMethod::RemoteAdd => {
                run_interactive("flatpak", &["remote-add", "--if-not-exists", name, url]).await
            }
            _ => Err(crate::error::PikeError::Other(format!(
                "flatpak does not support {} method",
                method
            ))),
        }
    }

    async fn remove_repo(&self, id: &str) -> Result<()> {
        run_interactive("flatpak", &["remote-delete", id]).await
    }
}

impl FlatpakSource {
    async fn resolve_app_id(&self, package: &str) -> Result<String> {
        if package.contains('.') {
            return Ok(package.to_string());
        }
        let results = self.search(package).await?;
        let query = package.to_lowercase();
        let matched = results
            .iter()
            .find(|p| p.name.to_lowercase().contains(&query));
        matched
            .map(|p| p.name.clone())
            .ok_or_else(|| crate::error::PikeError::NotFound {
                name: package.to_string(),
                source_name: "flatpak".to_string(),
            })
    }
}

fn non_empty(s: &str) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

fn parse_tab_lines<T>(output: &str, min_fields: usize, mapper: impl Fn(&[&str]) -> T) -> Vec<T> {
    output
        .lines()
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            let fields: Vec<&str> = line.split('\t').collect();
            (fields.len() >= min_fields).then(|| mapper(&fields))
        })
        .collect()
}

pub(crate) fn parse_search_output(output: &str) -> Vec<Package> {
    parse_tab_lines(output, 4, |f| Package {
        name: f[2].to_string(),
        version: f[3].to_string(),
        source: SourceType::Flatpak,
        arch: None,
        description: non_empty(f[1]),
    })
}

pub(crate) fn parse_updates_output(output: &str) -> Vec<PackageUpdate> {
    parse_tab_lines(output, 4, |f| PackageUpdate {
        name: f[1].to_string(),
        source: SourceType::Flatpak,
        arch: non_empty(f[3]),
        installed_version: String::new(),
        available_version: f[2].to_string(),
    })
}

pub(crate) fn parse_list_installed_output(output: &str) -> Vec<Package> {
    parse_tab_lines(output, 4, |f| Package {
        name: f[1].to_string(),
        version: f[2].to_string(),
        source: SourceType::Flatpak,
        arch: non_empty(f[3]),
        description: f.get(4).and_then(|s| non_empty(s)),
    })
}

pub(crate) fn parse_remotes_output(output: &str) -> Vec<Repository> {
    let mut repos = Vec::new();

    for line in output.lines() {
        if line.is_empty() {
            continue;
        }

        let fields: Vec<&str> = line.split('\t').collect();
        if fields.is_empty() {
            continue;
        }

        let id = fields[0].trim().to_string();
        if id.is_empty() {
            continue;
        }

        let name = fields.get(1).unwrap_or(&"").trim().to_string();
        let url = fields
            .get(2)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let options = fields.get(3).unwrap_or(&"");
        let enabled = !options.split(',').any(|opt| opt.trim() == "disabled");

        repos.push(Repository {
            id,
            name,
            source: SourceType::Flatpak,
            enabled,
            url,
        });
    }

    repos
}

#[cfg(test)]
mod tests {
    use super::*;

    const FLATPAK_SEARCH_OUTPUT: &str = "Firefox\tFast, private web browser\torg.mozilla.firefox\t136.0\tflathub\nGIMP\tGNU Image Manipulation Program\torg.gimp.GIMP\t2.10.38\tflathub\n";

    const FLATPAK_UPDATES_OUTPUT: &str = "Firefox\torg.mozilla.firefox\t137.0\tx86_64\n";

    #[test]
    fn test_parse_search() {
        let packages = parse_search_output(FLATPAK_SEARCH_OUTPUT);
        assert_eq!(packages.len(), 2);

        assert_eq!(packages[0].name, "org.mozilla.firefox");
        assert_eq!(packages[0].version, "136.0");
        assert_eq!(packages[0].source, SourceType::Flatpak);
        assert!(packages[0].arch.is_none());
        assert_eq!(
            packages[0].description.as_ref().unwrap(),
            "Fast, private web browser"
        );

        assert_eq!(packages[1].name, "org.gimp.GIMP");
        assert_eq!(packages[1].version, "2.10.38");
        assert!(packages[1].arch.is_none());
    }

    #[test]
    fn test_parse_updates() {
        let updates = parse_updates_output(FLATPAK_UPDATES_OUTPUT);
        assert_eq!(updates.len(), 1);

        assert_eq!(updates[0].name, "org.mozilla.firefox");
        assert_eq!(updates[0].available_version, "137.0");
        assert_eq!(updates[0].source, SourceType::Flatpak);
        assert_eq!(updates[0].arch.as_deref(), Some("x86_64"));
    }

    #[test]
    fn test_parse_installed_versions() {
        let output = "org.mozilla.firefox\t136.0\norg.gimp.GIMP\t2.10.38\n";
        let versions = parse_installed_versions(output, '\t');
        assert_eq!(versions.len(), 2);
        assert_eq!(versions["org.mozilla.firefox"], "136.0");
        assert_eq!(versions["org.gimp.GIMP"], "2.10.38");
    }

    const FLATPAK_LIST_INSTALLED_OUTPUT: &str = "Firefox\torg.mozilla.firefox\t136.0\tx86_64\tFast, private web browser\nGIMP\torg.gimp.GIMP\t2.10.38\tx86_64\tGNU Image Manipulation Program\n";

    #[test]
    fn test_parse_list_installed() {
        let packages = parse_list_installed_output(FLATPAK_LIST_INSTALLED_OUTPUT);
        assert_eq!(packages.len(), 2);

        assert_eq!(packages[0].name, "org.mozilla.firefox");
        assert_eq!(packages[0].version, "136.0");
        assert_eq!(packages[0].source, SourceType::Flatpak);
        assert_eq!(packages[0].arch.as_deref(), Some("x86_64"));
        assert_eq!(
            packages[0].description.as_ref().unwrap(),
            "Fast, private web browser"
        );

        assert_eq!(packages[1].name, "org.gimp.GIMP");
        assert_eq!(packages[1].version, "2.10.38");
        assert_eq!(packages[1].arch.as_deref(), Some("x86_64"));
    }

    #[test]
    fn test_parse_list_installed_empty() {
        assert!(parse_list_installed_output("").is_empty());
    }

    #[test]
    fn test_parse_empty() {
        assert!(parse_search_output("").is_empty());
        assert!(parse_updates_output("").is_empty());
    }

    const FLATPAK_REMOTES_OUTPUT: &str = "flathub\tFlathub\thttps://dl.flathub.org/repo/\t\nfedora\tFedora Flatpaks\thttps://flatpaks.fedora.org/repo/\tdisabled\n";

    #[test]
    fn test_parse_remotes() {
        let repos = parse_remotes_output(FLATPAK_REMOTES_OUTPUT);
        assert_eq!(repos.len(), 2);

        assert_eq!(repos[0].id, "flathub");
        assert_eq!(repos[0].name, "Flathub");
        assert_eq!(
            repos[0].url.as_deref(),
            Some("https://dl.flathub.org/repo/")
        );
        assert!(repos[0].enabled);
        assert_eq!(repos[0].source, SourceType::Flatpak);

        assert_eq!(repos[1].id, "fedora");
        assert!(!repos[1].enabled);
    }

    #[test]
    fn test_parse_remotes_empty() {
        assert!(parse_remotes_output("").is_empty());
    }
}
