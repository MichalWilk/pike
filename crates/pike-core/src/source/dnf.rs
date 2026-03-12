use async_trait::async_trait;

use crate::error::PikeError;
use crate::package::{Package, PackageUpdate, RepoMethod, Repository, SourceType};
use crate::source::{
    PackageSource, Result, parse_installed_versions, run_captured, run_captured_allow_exit,
    run_privileged,
};

fn version_key(name: &str, arch: Option<&str>) -> String {
    match arch {
        Some(a) => format!("{name}.{a}"),
        None => name.to_string(),
    }
}

#[derive(Default)]
pub struct DnfSource;

#[async_trait]
impl PackageSource for DnfSource {
    fn name(&self) -> &str {
        "dnf"
    }

    fn source_type(&self) -> SourceType {
        SourceType::Dnf
    }

    async fn search(&self, query: &str) -> Result<Vec<Package>> {
        let output = run_captured("dnf5", &["search", query]).await?;
        let mut packages = parse_search_output(&output);

        if !packages.is_empty() {
            let names: Vec<String> = packages
                .iter()
                .map(|p| version_key(&p.name, p.arch.as_deref()))
                .collect();
            let mut args = vec![
                "repoquery",
                "--latest-limit=1",
                "--queryformat=%{name}.%{arch}\t%{version}-%{release}\n",
                "-q",
            ];
            let name_refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
            args.extend_from_slice(&name_refs);
            if let Ok(ver_output) = run_captured("dnf5", &args).await {
                let versions = parse_installed_versions(&ver_output, '\t');
                for p in &mut packages {
                    let key = version_key(&p.name, p.arch.as_deref());
                    if let Some(ver) = versions.get(key.as_str()) {
                        p.version.clone_from(ver);
                    }
                }
            }
        }

        Ok(packages)
    }

    async fn install(&self, package: &str) -> Result<()> {
        run_privileged(&["dnf5", "install", "-y", package]).await
    }

    async fn install_many(&self, packages: &[String]) -> Result<()> {
        let mut args = vec!["dnf5", "install", "-y"];
        let refs: Vec<&str> = packages.iter().map(|s| s.as_str()).collect();
        args.extend_from_slice(&refs);
        run_privileged(&args).await
    }

    async fn remove(&self, package: &str, _purge: bool) -> Result<()> {
        run_privileged(&["dnf5", "remove", "-y", package]).await
    }

    async fn remove_many(&self, packages: &[String], _purge: bool) -> Result<()> {
        let mut args = vec!["dnf5", "remove", "-y"];
        let refs: Vec<&str> = packages.iter().map(|s| s.as_str()).collect();
        args.extend_from_slice(&refs);
        run_privileged(&args).await
    }

    async fn autoremove(&self) -> Result<()> {
        run_privileged(&["dnf5", "autoremove", "-y"]).await
    }

    async fn check_updates(&self) -> Result<Vec<PackageUpdate>> {
        let output = run_captured_allow_exit("dnf5", &["check-upgrade", "-q"], &[100]).await?;
        let mut updates = parse_check_upgrade_output(&output);

        let installed_output = run_captured(
            "dnf5",
            &[
                "repoquery",
                "--installed",
                "--queryformat=%{name}.%{arch}\t%{version}-%{release}\n",
                "-q",
            ],
        )
        .await?;
        let installed_versions = parse_installed_versions(&installed_output, '\t');
        for u in &mut updates {
            let key = version_key(&u.name, u.arch.as_deref());
            if let Some(ver) = installed_versions.get(key.as_str()) {
                u.installed_version.clone_from(ver);
            }
        }

        Ok(updates)
    }

    async fn update(&self, package: &str) -> Result<()> {
        run_privileged(&["dnf5", "upgrade", "-y", package]).await
    }

    async fn update_many(&self, packages: &[String]) -> Result<()> {
        let mut args = vec!["dnf5", "upgrade", "-y"];
        let refs: Vec<&str> = packages.iter().map(|s| s.as_str()).collect();
        args.extend_from_slice(&refs);
        run_privileged(&args).await
    }

    async fn update_all(&self) -> Result<()> {
        run_privileged(&["dnf5", "upgrade", "-y"]).await
    }

    async fn list_installed(&self) -> Result<Vec<Package>> {
        let output = run_captured(
            "dnf5",
            &[
                "repoquery",
                "--installed",
                "--queryformat=%{name}.%{arch}\t%{version}-%{release}\t%{summary}\n",
                "-q",
            ],
        )
        .await?;
        Ok(parse_list_installed_output(&output))
    }

    async fn list_repos(&self) -> Result<Vec<Repository>> {
        let output = run_captured("dnf5", &["repo", "list", "--all", "--json"]).await?;
        parse_repo_list_json(&output)
    }

    async fn set_repo_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        let value = if enabled { "1" } else { "0" };
        let opt = format!("{}.enabled={}", id, value);
        run_privileged(&["dnf5", "config-manager", "setopt", &opt]).await
    }

    async fn add_repo(
        &self,
        method: RepoMethod,
        repo_id: &str,
        name: &str,
        url: &str,
        gpgcheck: bool,
    ) -> Result<()> {
        match method {
            RepoMethod::RepoFile => {
                let arg = format!("--from-repofile={url}");
                dnf_addrepo(&arg, repo_id, name, gpgcheck).await
            }
            RepoMethod::Copr => run_privileged(&["dnf5", "copr", "enable", url]).await,
            RepoMethod::BaseUrl => {
                let arg = format!("--set=baseurl={url}");
                dnf_addrepo(&arg, repo_id, name, gpgcheck).await
            }
            RepoMethod::RpmPackage => run_privileged(&["dnf5", "install", url]).await,
            _ => Err(PikeError::Other(format!(
                "dnf does not support {} method",
                method
            ))),
        }
    }
}

async fn dnf_addrepo(url_arg: &str, repo_id: &str, name: &str, gpgcheck: bool) -> Result<()> {
    let id_opt = format!("--id={repo_id}");
    let name_opt = format!("--set=name={name}");
    let mut args = vec!["dnf5", "config-manager", "addrepo", url_arg];
    if !repo_id.is_empty() {
        args.push(&id_opt);
    }
    if !name.is_empty() {
        args.push(&name_opt);
    }
    if !gpgcheck {
        args.push("--set=gpgcheck=0");
    }
    run_privileged(&args).await
}

pub(crate) fn parse_search_output(output: &str) -> Vec<Package> {
    let mut packages = Vec::new();

    for line in output.lines() {
        if line.is_empty() {
            continue;
        }

        let trimmed = line.trim();
        if let Some((name_arch, description)) = trimmed.split_once('\t')
            && let Some((name, arch)) = extract_package_name_arch(name_arch)
        {
            packages.push(Package {
                name,
                version: String::new(),
                source: SourceType::Dnf,
                arch: Some(arch),
                description: Some(description.trim().to_string()),
            });
        }
    }

    packages
}

fn extract_package_name_arch(name_arch: &str) -> Option<(String, String)> {
    let (name, arch) = name_arch.rsplit_once('.')?;
    if arch != "src" && !SourceType::Dnf.known_arches().contains(&arch) {
        return None;
    }
    Some((name.to_string(), arch.to_string()))
}

fn parse_whitespace_lines<T>(
    output: &str,
    min_fields: usize,
    mapper: impl Fn(&[&str]) -> Option<T>,
) -> Vec<T> {
    output
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            (parts.len() >= min_fields).then(|| mapper(&parts))?
        })
        .collect()
}

pub(crate) fn parse_check_upgrade_output(output: &str) -> Vec<PackageUpdate> {
    parse_whitespace_lines(output, 2, |parts| {
        let (name, arch) = extract_package_name_arch(parts[0])?;
        Some(PackageUpdate {
            name,
            source: SourceType::Dnf,
            arch: Some(arch),
            installed_version: String::new(),
            available_version: parts[1].to_string(),
        })
    })
}

pub(crate) fn parse_list_installed_output(output: &str) -> Vec<Package> {
    output
        .lines()
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            let fields: Vec<&str> = line.split('\t').collect();
            if fields.len() < 2 {
                return None;
            }
            let (name, arch) = extract_package_name_arch(fields[0])?;
            Some(Package {
                name,
                version: fields[1].to_string(),
                source: SourceType::Dnf,
                arch: Some(arch),
                description: fields.get(2).and_then(|s| {
                    let s = s.trim();
                    if s.is_empty() {
                        None
                    } else {
                        Some(s.to_string())
                    }
                }),
            })
        })
        .collect()
}

pub(crate) fn parse_repo_list_json(output: &str) -> Result<Vec<Repository>> {
    let repos: Vec<serde_json::Value> =
        serde_json::from_str(output).map_err(|e| PikeError::Parse {
            source_name: "dnf".to_string(),
            detail: format!("invalid JSON from dnf5 repo list: {}", e),
        })?;

    let mut result = Vec::new();
    for entry in repos {
        let id = entry["id"].as_str().unwrap_or_default().to_string();
        let name = entry["name"].as_str().unwrap_or_default().to_string();
        let enabled = entry["is_enabled"].as_bool().unwrap_or(false);

        if id.is_empty() {
            continue;
        }

        result.push(Repository {
            id,
            name,
            source: SourceType::Dnf,
            enabled,
            url: None,
        });
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DNF_SEARCH_OUTPUT: &str = "Updating and loading repositories:\nRepositories loaded.\nMatched fields: name (exact)\n lan-mouse.aarch64\tSoftware KVM Switch / mouse & keyboard sharing software\n lan-mouse.x86_64\tSoftware KVM Switch / mouse & keyboard sharing software\nMatched fields: name, summary\n lan-mouse-debuginfo.x86_64\tDebug information for package lan-mouse\n";

    const DNF_CHECK_UPGRADE_OUTPUT: &str = " bash.x86_64                  5.2.38-1.fc43                    updates\n vim-enhanced.x86_64          9.1.900-1.fc43                   updates\n";

    #[test]
    fn test_parse_search() {
        let packages = parse_search_output(DNF_SEARCH_OUTPUT);
        assert_eq!(packages.len(), 3);

        assert_eq!(packages[0].name, "lan-mouse");
        assert_eq!(packages[0].arch.as_deref(), Some("aarch64"));

        assert_eq!(packages[1].name, "lan-mouse");
        assert_eq!(packages[1].arch.as_deref(), Some("x86_64"));

        assert_eq!(packages[2].name, "lan-mouse-debuginfo");
        assert_eq!(packages[2].arch.as_deref(), Some("x86_64"));
    }

    #[test]
    fn test_parse_search_skips_headers() {
        let output =
            "Updating and loading repositories:\nRepositories loaded.\nMatched fields: name\n";
        let packages = parse_search_output(output);
        assert!(packages.is_empty());
    }

    #[test]
    fn test_parse_check_upgrade() {
        let updates = parse_check_upgrade_output(DNF_CHECK_UPGRADE_OUTPUT);
        assert_eq!(updates.len(), 2);

        assert_eq!(updates[0].name, "bash");
        assert_eq!(updates[0].arch.as_deref(), Some("x86_64"));
        assert_eq!(updates[0].available_version, "5.2.38-1.fc43");
        assert_eq!(updates[0].source, SourceType::Dnf);

        assert_eq!(updates[1].name, "vim-enhanced");
        assert_eq!(updates[1].arch.as_deref(), Some("x86_64"));
        assert_eq!(updates[1].available_version, "9.1.900-1.fc43");
    }

    #[test]
    fn test_parse_empty() {
        assert!(parse_search_output("").is_empty());
        assert!(parse_check_upgrade_output("").is_empty());
    }

    const DNF_LIST_INSTALLED_OUTPUT: &str = "bash.x86_64\t5.2.37-3.fc43\tThe GNU Bourne Again shell\nvim-enhanced.x86_64\t9.1.900-1.fc43\tA version of the VIM editor\n";

    #[test]
    fn test_parse_list_installed() {
        let packages = parse_list_installed_output(DNF_LIST_INSTALLED_OUTPUT);
        assert_eq!(packages.len(), 2);

        assert_eq!(packages[0].name, "bash");
        assert_eq!(packages[0].arch.as_deref(), Some("x86_64"));
        assert_eq!(packages[0].version, "5.2.37-3.fc43");
        assert_eq!(packages[0].source, SourceType::Dnf);
        assert_eq!(
            packages[0].description.as_deref(),
            Some("The GNU Bourne Again shell")
        );

        assert_eq!(packages[1].name, "vim-enhanced");
        assert_eq!(packages[1].version, "9.1.900-1.fc43");
        assert_eq!(
            packages[1].description.as_deref(),
            Some("A version of the VIM editor")
        );
    }

    #[test]
    fn test_parse_list_installed_empty() {
        assert!(parse_list_installed_output("").is_empty());
    }

    #[test]
    fn test_parse_search_multiline_description() {
        let output = " ripgrep.x86_64\tLine-oriented search tool\n";
        let packages = parse_search_output(output);
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name, "ripgrep");
        assert!(
            packages[0]
                .description
                .as_ref()
                .unwrap()
                .contains("Line-oriented")
        );
    }

    const DNF_REPO_LIST_JSON: &str = r#"[
        {"id": "fedora", "name": "Fedora 43 - x86_64", "is_enabled": true},
        {"id": "updates", "name": "Fedora 43 - x86_64 - Updates", "is_enabled": true},
        {"id": "updates-testing", "name": "Fedora 43 - x86_64 - Test Updates", "is_enabled": false}
    ]"#;

    #[test]
    fn test_parse_repo_list_json() {
        let repos = parse_repo_list_json(DNF_REPO_LIST_JSON).unwrap();
        assert_eq!(repos.len(), 3);

        assert_eq!(repos[0].id, "fedora");
        assert_eq!(repos[0].name, "Fedora 43 - x86_64");
        assert!(repos[0].enabled);
        assert_eq!(repos[0].source, SourceType::Dnf);
        assert!(repos[0].url.is_none());

        assert_eq!(repos[2].id, "updates-testing");
        assert!(!repos[2].enabled);
    }

    #[test]
    fn test_parse_repo_list_json_empty() {
        let repos = parse_repo_list_json("[]").unwrap();
        assert!(repos.is_empty());
    }

    #[test]
    fn test_parse_repo_list_json_invalid() {
        let result = parse_repo_list_json("not json");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_dotted_package_names() {
        let output = " python3.11.x86_64\tPython 3.11 interpreter\n python3.12.x86_64\tPython 3.12 interpreter\n";
        let packages = parse_search_output(output);
        assert_eq!(packages.len(), 2);
        assert_eq!(packages[0].name, "python3.11");
        assert_eq!(packages[0].arch.as_deref(), Some("x86_64"));
        assert_eq!(packages[1].name, "python3.12");
        assert_eq!(packages[1].arch.as_deref(), Some("x86_64"));
    }

    #[test]
    fn test_parse_dotted_package_check_upgrade() {
        let output = " python3.11.x86_64              3.11.12-1.fc43                   updates\n";
        let updates = parse_check_upgrade_output(output);
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].name, "python3.11");
        assert_eq!(updates[0].arch.as_deref(), Some("x86_64"));
        assert_eq!(updates[0].available_version, "3.11.12-1.fc43");
    }

    #[test]
    fn test_parse_dotted_package_list_installed() {
        let output = "python3.11.x86_64\t3.11.11-1.fc43\tPython 3.11 interpreter\n";
        let packages = parse_list_installed_output(output);
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name, "python3.11");
        assert_eq!(packages[0].arch.as_deref(), Some("x86_64"));
        assert_eq!(packages[0].version, "3.11.11-1.fc43");
    }
}
