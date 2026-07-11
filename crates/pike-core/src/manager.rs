use std::collections::BTreeMap;

use crate::config::Config;
use crate::db::Database;
use crate::error::PikeError;
use crate::package::{Package, PackageUpdate, RepoMethod, Repository, SourceType, StatusSummary};
use crate::source::{PackageSource, PendingGpgKey, create_sources};
use crate::util::{filter_and_sort_packages, gather, sort_by_source};

pub struct PackageManager {
    config: Config,
    db: Database,
    sources: Vec<Box<dyn PackageSource>>,
}

impl PackageManager {
    pub async fn new(config: Config, db: Database) -> Result<Self, PikeError> {
        let mut active = Vec::new();
        for &st in SourceType::ALL {
            let name = st.binary_name();
            if config.sources.enabled(st) && binary_exists(name).await {
                tracing::debug!("{name} found and enabled");
                active.push(st);
            } else if config.sources.enabled(st) {
                tracing::warn!("{name} enabled in config but not found on system");
            } else {
                tracing::debug!("{name} disabled in config");
            }
        }

        if active.is_empty() {
            return Err(PikeError::Other(
                "no package managers found (need dnf5, apt-get, or flatpak)".into(),
            ));
        }

        Ok(Self {
            config,
            db,
            sources: create_sources(&active),
        })
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn active_source_types(&self) -> Vec<SourceType> {
        self.sources.iter().map(|s| s.source_type()).collect()
    }

    pub fn active_source_names(&self) -> Vec<&str> {
        self.sources.iter().map(|s| s.name()).collect()
    }

    pub fn cache_updates(&self, updates: &[PackageUpdate]) -> Result<(), PikeError> {
        self.db.replace_cache(updates)
    }

    pub async fn search(
        &self,
        query: &str,
        source_filter: Option<SourceType>,
    ) -> Result<Vec<Package>, PikeError> {
        let sources = self.filtered_sources(source_filter);
        let futures: Vec<_> = sources.iter().map(|s| s.search(query)).collect();
        let mut packages = gather(futures, "search").await;

        filter_and_sort_packages(&mut packages, &self.config);
        Ok(packages)
    }

    pub async fn install(
        &self,
        package: &str,
        source: Option<SourceType>,
    ) -> Result<String, PikeError> {
        let s = self.resolve_package_source(package, source).await?;
        s.install(package).await?;
        Ok(s.name().to_string())
    }

    pub async fn install_many(
        &self,
        packages: &[String],
        source: Option<SourceType>,
    ) -> Result<Vec<(String, Vec<String>)>, PikeError> {
        let groups = self.resolve_source_groups(packages, source).await?;
        let mut result = Vec::new();
        for (s, pkgs) in &groups {
            s.install_many(pkgs).await?;
            result.push((s.name().to_string(), pkgs.clone()));
        }
        Ok(result)
    }

    pub async fn remove(
        &self,
        package: &str,
        source: Option<SourceType>,
        purge: bool,
    ) -> Result<String, PikeError> {
        let s = self.resolve_package_source(package, source).await?;
        s.remove(package, purge).await?;
        Ok(s.name().to_string())
    }

    pub async fn remove_many(
        &self,
        packages: &[String],
        source: Option<SourceType>,
        purge: bool,
    ) -> Result<Vec<(String, Vec<String>)>, PikeError> {
        let groups = self.resolve_source_groups(packages, source).await?;
        let mut result = Vec::new();
        for (s, pkgs) in &groups {
            s.remove_many(pkgs, purge).await?;
            result.push((s.name().to_string(), pkgs.clone()));
        }
        Ok(result)
    }

    pub async fn autoremove_source(&self, source: SourceType) -> Result<(), PikeError> {
        self.get_source(source)?.autoremove().await
    }

    pub async fn update_package(&self, package: &str) -> Result<(), PikeError> {
        for s in &self.sources {
            match s.update(package).await {
                Ok(()) => return Ok(()),
                Err(PikeError::NotFound { .. }) => continue,
                Err(e) => return Err(e),
            }
        }
        Err(PikeError::NotFound {
            name: package.to_string(),
            source_name: "any source".to_string(),
        })
    }

    pub async fn update_many(
        &self,
        packages: &[String],
        source: Option<SourceType>,
    ) -> Result<Vec<(String, Vec<String>)>, PikeError> {
        let groups = self.resolve_source_groups(packages, source).await?;
        let mut result = Vec::new();
        for (s, pkgs) in &groups {
            s.update_many(pkgs).await?;
            result.push((s.name().to_string(), pkgs.clone()));
        }
        Ok(result)
    }

    pub async fn update_source(&self, package: &str, source: SourceType) -> Result<(), PikeError> {
        self.get_source(source)?.update(package).await
    }

    pub async fn update_all_source(&self, source: SourceType) -> Result<(), PikeError> {
        self.get_source(source)?.update_all().await
    }

    pub async fn check_updates(&self) -> Result<Vec<PackageUpdate>, PikeError> {
        let futures: Vec<_> = self.sources.iter().map(|s| s.check_updates()).collect();
        let updates = gather(futures, "check_updates").await;

        self.db.replace_cache(&updates)?;

        Ok(updates)
    }

    pub async fn refresh_preflight(&self) -> Vec<(SourceType, Vec<PendingGpgKey>)> {
        let mut pending = Vec::new();
        for source in &self.sources {
            match source.refresh_preflight().await {
                Ok(keys) if !keys.is_empty() => pending.push((source.source_type(), keys)),
                Ok(_) => {}
                Err(e) => tracing::warn!("refresh_preflight {} failed: {}", source.name(), e),
            }
        }
        pending
    }

    pub async fn import_keys(&self, source: SourceType) -> Result<(), PikeError> {
        self.get_source(source)?.import_keys().await
    }

    pub async fn list_installed(
        &self,
        source_filter: Option<SourceType>,
    ) -> Result<Vec<Package>, PikeError> {
        let sources = self.filtered_sources(source_filter);
        let futures: Vec<_> = sources.iter().map(|s| s.list_installed()).collect();
        let mut packages = gather(futures, "list_installed").await;

        sort_by_source(&mut packages, |p| &p.source, |p| &p.name);
        Ok(packages)
    }

    pub fn get_cached_status(&self) -> Result<StatusSummary, PikeError> {
        let updates = self.db.get_cached_updates()?;
        let checked_at = self.db.get_last_checked()?;
        let mut status = StatusSummary::from_updates(updates);
        status.checked_at = checked_at;
        Ok(status)
    }

    pub async fn list_repos(
        &self,
        source_filter: Option<SourceType>,
    ) -> Result<Vec<Repository>, PikeError> {
        let sources = self.filtered_sources(source_filter);
        let futures: Vec<_> = sources.iter().map(|s| s.list_repos()).collect();
        let mut repos = gather(futures, "list_repos").await;

        sort_by_source(&mut repos, |r| &r.source, |r| r.id.as_str());
        Ok(repos)
    }

    pub async fn set_repo_enabled(
        &self,
        id: &str,
        enabled: bool,
        source: Option<SourceType>,
    ) -> Result<(), PikeError> {
        let s = self.resolve_repo_source(id, source).await?;
        s.set_repo_enabled(id, enabled).await
    }

    pub async fn add_repo(
        &self,
        method: RepoMethod,
        repo_id: &str,
        name: &str,
        url: &str,
        source: SourceType,
        gpgcheck: bool,
    ) -> Result<(), PikeError> {
        validate_repo_input(method, repo_id, name, url)?;
        let s = self.get_source(source)?;
        s.add_repo(method, repo_id, name, url, gpgcheck).await
    }

    pub async fn remove_repo(&self, id: &str, source: Option<SourceType>) -> Result<(), PikeError> {
        let s = self.resolve_repo_source(id, source).await?;
        s.remove_repo(id).await
    }

    fn filtered_sources(&self, filter: Option<SourceType>) -> Vec<&dyn PackageSource> {
        match filter {
            Some(st) => self
                .sources
                .iter()
                .filter(|s| s.source_type() == st)
                .map(|s| s.as_ref())
                .collect(),
            None => self.sources.iter().map(|s| s.as_ref()).collect(),
        }
    }

    fn get_source(&self, source_type: SourceType) -> Result<&dyn PackageSource, PikeError> {
        self.sources
            .iter()
            .find(|s| s.source_type() == source_type)
            .map(|s| s.as_ref())
            .ok_or_else(|| PikeError::Other(format!("{} source not available", source_type)))
    }

    async fn resolve_source_groups(
        &self,
        packages: &[String],
        source: Option<SourceType>,
    ) -> Result<Vec<(&dyn PackageSource, Vec<String>)>, PikeError> {
        match source {
            Some(st) => {
                let s = self.get_source(st)?;
                Ok(vec![(s, packages.to_vec())])
            }
            None => {
                let mut grouped: BTreeMap<SourceType, Vec<String>> = BTreeMap::new();
                for pkg in packages {
                    let s = self.resolve_package_source(pkg, None).await?;
                    grouped
                        .entry(s.source_type())
                        .or_default()
                        .push(pkg.clone());
                }
                grouped
                    .into_iter()
                    .map(|(st, pkgs)| Ok((self.get_source(st)?, pkgs)))
                    .collect()
            }
        }
    }

    async fn resolve_package_source(
        &self,
        package: &str,
        source: Option<SourceType>,
    ) -> Result<&dyn PackageSource, PikeError> {
        match source {
            Some(src) => self.get_source(src),
            None => self.find_package_source(package).await,
        }
    }

    async fn resolve_repo_source(
        &self,
        id: &str,
        source: Option<SourceType>,
    ) -> Result<&dyn PackageSource, PikeError> {
        match source {
            Some(src) => self.get_source(src),
            None => self.find_repo_source(id).await,
        }
    }

    async fn find_repo_source(&self, id: &str) -> Result<&dyn PackageSource, PikeError> {
        let futures: Vec<_> = self.sources.iter().map(|s| s.list_repos()).collect();
        let results = futures::future::join_all(futures).await;

        for (i, result) in results.into_iter().enumerate() {
            match result {
                Ok(repos) if repos.iter().any(|r| r.id == id) => {
                    return Ok(self.sources[i].as_ref());
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!("{} repo lookup failed: {e}", self.sources[i].name());
                }
            }
        }
        Err(PikeError::NotFound {
            name: id.to_string(),
            source_name: "any source".to_string(),
        })
    }

    async fn find_package_source(&self, package: &str) -> Result<&dyn PackageSource, PikeError> {
        let futures: Vec<_> = self.sources.iter().map(|s| s.search(package)).collect();
        let results = futures::future::join_all(futures).await;

        let query = package.to_lowercase();
        let mut found_in: Vec<&dyn PackageSource> = Vec::new();
        for (i, result) in results.into_iter().enumerate() {
            match result {
                Ok(pkgs) if pkgs.iter().any(|p| p.name.to_lowercase().contains(&query)) => {
                    found_in.push(self.sources[i].as_ref());
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!("{} search failed: {e}", self.sources[i].name());
                }
            }
        }

        match found_in.len() {
            0 => Err(PikeError::NotFound {
                name: package.to_string(),
                source_name: "any source".to_string(),
            }),
            1 => Ok(found_in[0]),
            _ => Err(PikeError::Other(format!(
                "'{}' found in multiple sources. Use --source to specify: {}",
                package,
                found_in
                    .iter()
                    .map(|s| s.name())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
        }
    }
}

fn has_valid_url_scheme(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://") || url.starts_with("file://")
}

pub fn validate_repo_input(
    method: RepoMethod,
    repo_id: &str,
    name: &str,
    url: &str,
) -> Result<(), PikeError> {
    if !repo_id.is_empty()
        && !repo_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
    {
        return Err(PikeError::Validation(
            "repo ID may only contain letters, digits, '.', '_', '-'".into(),
        ));
    }

    if name.chars().any(|c| c.is_control()) {
        return Err(PikeError::Validation(
            "repo name must not contain control characters".into(),
        ));
    }

    match method {
        RepoMethod::Copr => {
            if url.is_empty() {
                return Err(PikeError::Validation(
                    "Copr identifier is required (owner/project)".into(),
                ));
            }
            let parts: Vec<&str> = url.splitn(2, '/').collect();
            if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
                return Err(PikeError::Validation(
                    "Copr identifier must be in owner/project format".into(),
                ));
            }
        }
        RepoMethod::RpmPackage => {
            if !url.ends_with(".rpm") {
                return Err(PikeError::Validation("RPM URL must end in .rpm".into()));
            }
            if !has_valid_url_scheme(url) {
                return Err(PikeError::Validation(
                    "RPM URL must start with http://, https://, or file://".into(),
                ));
            }
        }
        RepoMethod::Ppa => {
            if url.is_empty() {
                return Err(PikeError::Validation(
                    "PPA identifier is required (user/ppa-name)".into(),
                ));
            }
            let parts: Vec<&str> = url.splitn(2, '/').collect();
            if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
                return Err(PikeError::Validation(
                    "PPA identifier must be in user/ppa-name format".into(),
                ));
            }
        }
        _ => {
            if !has_valid_url_scheme(url) {
                return Err(PikeError::Validation(
                    "URL must start with http://, https://, or file://".into(),
                ));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_repo_copr_empty_url_rejected() {
        let result = validate_repo_input(RepoMethod::Copr, "", "", "");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_repo_rpm_empty_url_rejected() {
        let result = validate_repo_input(RepoMethod::RpmPackage, "", "", "");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_repo_copr_valid() {
        let result = validate_repo_input(RepoMethod::Copr, "", "", "owner/project");
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_repo_ppa_empty_url_rejected() {
        let result = validate_repo_input(RepoMethod::Ppa, "", "", "");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_repo_ppa_valid() {
        let result = validate_repo_input(RepoMethod::Ppa, "", "", "user/ppa-name");
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_repo_ppa_no_slash_rejected() {
        let result = validate_repo_input(RepoMethod::Ppa, "", "", "noslash");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_repo_ppa_empty_owner_rejected() {
        let result = validate_repo_input(RepoMethod::Ppa, "", "", "/ppa-name");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_repo_ppa_empty_name_rejected() {
        let result = validate_repo_input(RepoMethod::Ppa, "", "", "user/");
        assert!(result.is_err());
    }
}

async fn binary_exists(name: &str) -> bool {
    tokio::process::Command::new("which")
        .arg(name)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .is_ok_and(|s| s.success())
}
