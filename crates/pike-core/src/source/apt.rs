use async_trait::async_trait;

use crate::error::PikeError;
use crate::package::{Package, PackageUpdate, RepoMethod, Repository, SourceType};
use crate::source::{PackageSource, Result, run_captured, run_captured_allow_exit, run_privileged};

#[derive(Default)]
pub struct AptSource;

#[async_trait]
impl PackageSource for AptSource {
    fn name(&self) -> &str {
        "apt"
    }

    fn source_type(&self) -> SourceType {
        SourceType::Apt
    }

    async fn search(&self, query: &str) -> Result<Vec<Package>> {
        let output = run_captured("apt-cache", &["search", query]).await?;
        Ok(parse_search_output(&output))
    }

    async fn install(&self, package: &str) -> Result<()> {
        run_privileged(&["apt-get", "install", "-y", package]).await
    }

    async fn install_many(&self, packages: &[String]) -> Result<()> {
        let mut args = vec!["apt-get", "install", "-y"];
        let refs: Vec<&str> = packages.iter().map(|s| s.as_str()).collect();
        args.extend_from_slice(&refs);
        run_privileged(&args).await
    }

    async fn remove(&self, package: &str, purge: bool) -> Result<()> {
        if purge {
            run_privileged(&["apt-get", "purge", "-y", package]).await
        } else {
            run_privileged(&["apt-get", "remove", "-y", package]).await
        }
    }

    async fn remove_many(&self, packages: &[String], purge: bool) -> Result<()> {
        let cmd = if purge { "purge" } else { "remove" };
        let mut args = vec!["apt-get", cmd, "-y"];
        let refs: Vec<&str> = packages.iter().map(|s| s.as_str()).collect();
        args.extend_from_slice(&refs);
        run_privileged(&args).await
    }

    async fn autoremove(&self) -> Result<()> {
        run_privileged(&["apt-get", "autoremove", "-y"]).await
    }

    async fn check_updates(&self) -> Result<Vec<PackageUpdate>> {
        run_privileged(&["apt-get", "update", "-qq"]).await?;
        let output = run_captured("sh", &["-c", "LC_ALL=C apt-get -s upgrade"]).await?;
        Ok(parse_check_updates_output(&output))
    }

    async fn update(&self, package: &str) -> Result<()> {
        run_privileged(&["apt-get", "update", "-qq"]).await?;
        run_privileged(&["apt-get", "install", "--only-upgrade", "-y", package]).await
    }

    async fn update_many(&self, packages: &[String]) -> Result<()> {
        run_privileged(&["apt-get", "update", "-qq"]).await?;
        let mut args = vec!["apt-get", "install", "--only-upgrade", "-y"];
        let refs: Vec<&str> = packages.iter().map(|s| s.as_str()).collect();
        args.extend_from_slice(&refs);
        run_privileged(&args).await
    }

    async fn update_all(&self) -> Result<()> {
        run_privileged(&["apt-get", "update", "-qq"]).await?;
        run_privileged(&["apt-get", "upgrade", "-y"]).await
    }

    async fn list_installed(&self) -> Result<Vec<Package>> {
        let output = run_captured(
            "dpkg-query",
            &[
                "-W",
                "-f",
                "${Package}\t${Version}\t${Architecture}\t${binary:Summary}\n",
            ],
        )
        .await?;
        Ok(parse_list_installed_output(&output))
    }

    async fn list_repos(&self) -> Result<Vec<Repository>> {
        let (legacy, deb822) = tokio::join!(
            run_captured_allow_exit(
                "grep",
                &[
                    "-rh",
                    "^deb ",
                    "/etc/apt/sources.list",
                    "/etc/apt/sources.list.d/"
                ],
                &[1, 2],
            ),
            run_captured_allow_exit("sh", &["-c", "cat /etc/apt/sources.list.d/*.sources"], &[1],),
        );
        let mut repos = parse_sources_list(&legacy?);
        repos.extend(parse_deb822_sources(&deb822?));
        Ok(repos)
    }

    async fn add_repo(
        &self,
        method: RepoMethod,
        _repo_id: &str,
        _name: &str,
        url: &str,
        _gpgcheck: bool,
    ) -> Result<()> {
        match method {
            RepoMethod::Ppa => {
                let ppa = format!("ppa:{url}");
                run_privileged(&["add-apt-repository", "-y", &ppa]).await
            }
            RepoMethod::BaseUrl => run_privileged(&["add-apt-repository", "-y", url]).await,
            _ => Err(PikeError::Other(format!(
                "apt does not support {} method",
                method
            ))),
        }
    }

    async fn remove_repo(&self, id: &str) -> Result<()> {
        run_privileged(&["add-apt-repository", "--remove", "-y", id]).await
    }
}

pub(crate) fn parse_search_output(output: &str) -> Vec<Package> {
    output
        .lines()
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            let (name, description) = line.split_once(" - ")?;
            Some(Package {
                name: name.trim().to_string(),
                version: String::new(),
                source: SourceType::Apt,
                arch: None,
                description: Some(description.trim().to_string()),
            })
        })
        .collect()
}

pub(crate) fn parse_check_updates_output(output: &str) -> Vec<PackageUpdate> {
    output
        .lines()
        .filter(|line| line.starts_with("Inst "))
        .filter_map(|line| {
            let rest = &line[5..];
            let parts: Vec<&str> = rest.splitn(2, ' ').collect();
            if parts.len() < 2 {
                return None;
            }
            let name = parts[0];
            let remainder = parts[1];

            let (installed_version, paren_part) = if remainder.starts_with('[') {
                let bracket_end = remainder.find(']')?;
                let installed = remainder[1..bracket_end].to_string();
                let after_bracket = remainder[bracket_end + 1..].trim();
                (installed, after_bracket)
            } else {
                (String::new(), remainder)
            };

            if !paren_part.starts_with('(') {
                return None;
            }
            let paren_end = paren_part.find(')')?;
            let inner = &paren_part[1..paren_end];
            let inner_parts: Vec<&str> = inner.split_whitespace().collect();
            if inner_parts.is_empty() {
                return None;
            }

            let available_version = inner_parts[0].to_string();
            let arch = inner_parts.last().and_then(|s| {
                if s.starts_with('[') && s.ends_with(']') {
                    Some(s[1..s.len() - 1].to_string())
                } else {
                    None
                }
            });

            Some(PackageUpdate {
                name: name.to_string(),
                source: SourceType::Apt,
                arch,
                installed_version,
                available_version,
            })
        })
        .collect()
}

pub(crate) fn parse_list_installed_output(output: &str) -> Vec<Package> {
    output
        .lines()
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            let fields: Vec<&str> = line.split('\t').collect();
            if fields.len() < 3 {
                return None;
            }
            Some(Package {
                name: fields[0].to_string(),
                version: fields[1].to_string(),
                source: SourceType::Apt,
                arch: Some(fields[2].to_string()),
                description: fields.get(3).and_then(|s| {
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

pub(crate) fn parse_sources_list(output: &str) -> Vec<Repository> {
    output
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty() && !trimmed.starts_with('#') && trimmed.starts_with("deb ")
        })
        .filter_map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 2 {
                return None;
            }
            let url = parts[1].to_string();
            let id = parts[1..].join(" ");
            Some(Repository {
                id,
                name: url.clone(),
                source: SourceType::Apt,
                enabled: true,
                url: Some(url),
            })
        })
        .collect()
}

pub(crate) fn parse_deb822_sources(output: &str) -> Vec<Repository> {
    let mut repos = Vec::new();
    for stanza in output.split("\n\n") {
        let mut types = "";
        let mut uris = "";
        let mut suites = "";
        let mut components = "";
        let mut enabled = true;

        for line in stanza.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let lower = line.to_ascii_lowercase();
            if lower.starts_with("types:") {
                types = line[6..].trim();
            } else if lower.starts_with("uris:") {
                uris = line[5..].trim();
            } else if lower.starts_with("suites:") {
                suites = line[7..].trim();
            } else if lower.starts_with("components:") {
                components = line[11..].trim();
            } else if lower.starts_with("enabled:") {
                enabled = line[8..].trim() != "no";
            }
        }

        let has_deb = types.split_whitespace().any(|t| t == "deb");
        if !has_deb || uris.is_empty() {
            continue;
        }

        for uri in uris.split_whitespace() {
            let id = [uri, suites, components]
                .iter()
                .filter(|s| !s.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join(" ");
            repos.push(Repository {
                id,
                name: uri.to_string(),
                source: SourceType::Apt,
                enabled,
                url: Some(uri.to_string()),
            });
        }
    }
    repos
}

#[cfg(test)]
mod tests {
    use super::*;

    const APT_SEARCH_OUTPUT: &str = "firefox - Safe and easy web browser from Mozilla\nfirefox-locale-en - English language pack for Firefox\nchromium - open-source version of Chrome\n";

    #[test]
    fn test_parse_search() {
        let packages = parse_search_output(APT_SEARCH_OUTPUT);
        assert_eq!(packages.len(), 3);

        assert_eq!(packages[0].name, "firefox");
        assert_eq!(
            packages[0].description.as_deref(),
            Some("Safe and easy web browser from Mozilla")
        );
        assert_eq!(packages[0].source, SourceType::Apt);

        assert_eq!(packages[1].name, "firefox-locale-en");
        assert_eq!(
            packages[1].description.as_deref(),
            Some("English language pack for Firefox")
        );

        assert_eq!(packages[2].name, "chromium");
        assert_eq!(
            packages[2].description.as_deref(),
            Some("open-source version of Chrome")
        );
    }

    #[test]
    fn test_parse_search_empty() {
        assert!(parse_search_output("").is_empty());
    }

    const APT_CHECK_UPDATES_OUTPUT: &str = "Inst bash [5.2.21-2] (5.2.21-3 Ubuntu:24.04/noble-updates [amd64])\nInst vim [2:9.1.0-1] (2:9.1.0-2 Ubuntu:24.04/noble-updates [amd64])\nConf bash (5.2.21-3 Ubuntu:24.04/noble-updates [amd64])\nConf vim (2:9.1.0-2 Ubuntu:24.04/noble-updates [amd64])\n";

    #[test]
    fn test_parse_check_updates() {
        let updates = parse_check_updates_output(APT_CHECK_UPDATES_OUTPUT);
        assert_eq!(updates.len(), 2);

        assert_eq!(updates[0].name, "bash");
        assert_eq!(updates[0].installed_version, "5.2.21-2");
        assert_eq!(updates[0].available_version, "5.2.21-3");
        assert_eq!(updates[0].arch.as_deref(), Some("amd64"));
        assert_eq!(updates[0].source, SourceType::Apt);

        assert_eq!(updates[1].name, "vim");
        assert_eq!(updates[1].installed_version, "2:9.1.0-1");
        assert_eq!(updates[1].available_version, "2:9.1.0-2");
        assert_eq!(updates[1].arch.as_deref(), Some("amd64"));
    }

    #[test]
    fn test_parse_check_updates_no_old_version() {
        let input = "Inst newpkg (1.0-1 Ubuntu:24.04/noble [amd64])\nConf newpkg (1.0-1 Ubuntu:24.04/noble [amd64])\n";
        let updates = parse_check_updates_output(input);
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].name, "newpkg");
        assert_eq!(updates[0].installed_version, "");
        assert_eq!(updates[0].available_version, "1.0-1");
        assert_eq!(updates[0].arch.as_deref(), Some("amd64"));
    }

    #[test]
    fn test_parse_check_updates_empty() {
        assert!(parse_check_updates_output("").is_empty());
    }

    const APT_LIST_INSTALLED_OUTPUT: &str = "bash\t5.2.21-2\tamd64\tGNU Bourne Again SHell\nvim\t2:9.1.0-1\tamd64\tVi IMproved - enhanced vi editor\ncoreutils\t9.4-2\tamd64\tGNU core utilities\n";

    #[test]
    fn test_parse_list_installed() {
        let packages = parse_list_installed_output(APT_LIST_INSTALLED_OUTPUT);
        assert_eq!(packages.len(), 3);

        assert_eq!(packages[0].name, "bash");
        assert_eq!(packages[0].version, "5.2.21-2");
        assert_eq!(packages[0].arch.as_deref(), Some("amd64"));
        assert_eq!(packages[0].source, SourceType::Apt);
        assert_eq!(
            packages[0].description.as_deref(),
            Some("GNU Bourne Again SHell")
        );

        assert_eq!(packages[1].name, "vim");
        assert_eq!(packages[1].version, "2:9.1.0-1");
        assert_eq!(
            packages[1].description.as_deref(),
            Some("Vi IMproved - enhanced vi editor")
        );

        assert_eq!(packages[2].name, "coreutils");
        assert_eq!(packages[2].version, "9.4-2");
        assert_eq!(
            packages[2].description.as_deref(),
            Some("GNU core utilities")
        );
    }

    #[test]
    fn test_parse_list_installed_empty() {
        assert!(parse_list_installed_output("").is_empty());
    }

    const APT_SOURCES_OUTPUT: &str = "deb http://archive.ubuntu.com/ubuntu noble main restricted\ndeb http://archive.ubuntu.com/ubuntu noble-updates main restricted\ndeb http://ppa.launchpad.net/user/ppa-name/ubuntu noble main\n";

    #[test]
    fn test_parse_sources_list() {
        let repos = parse_sources_list(APT_SOURCES_OUTPUT);
        assert_eq!(repos.len(), 3);

        assert_eq!(
            repos[0].id,
            "http://archive.ubuntu.com/ubuntu noble main restricted"
        );
        assert_eq!(repos[0].name, "http://archive.ubuntu.com/ubuntu");
        assert_eq!(
            repos[0].url.as_deref(),
            Some("http://archive.ubuntu.com/ubuntu")
        );
        assert!(repos[0].enabled);
        assert_eq!(repos[0].source, SourceType::Apt);

        assert_eq!(
            repos[1].id,
            "http://archive.ubuntu.com/ubuntu noble-updates main restricted"
        );

        assert_eq!(
            repos[2].id,
            "http://ppa.launchpad.net/user/ppa-name/ubuntu noble main"
        );
    }

    #[test]
    fn test_parse_sources_list_empty() {
        assert!(parse_sources_list("").is_empty());
    }

    #[test]
    fn test_parse_sources_list_skips_comments() {
        let input = "# A comment\ndeb http://example.com/repo stable main\n# deb-src http://example.com/repo stable main\n";
        let repos = parse_sources_list(input);
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].name, "http://example.com/repo");
    }

    const DEB822_SOURCES: &str = "\
Types: deb
URIs: http://archive.ubuntu.com/ubuntu/
Suites: noble noble-updates noble-backports
Components: main universe restricted multiverse
Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg

Types: deb
URIs: http://security.ubuntu.com/ubuntu/
Suites: noble-security
Components: main universe restricted multiverse
Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg
";

    #[test]
    fn test_parse_deb822_sources() {
        let repos = parse_deb822_sources(DEB822_SOURCES);
        assert_eq!(repos.len(), 2);

        assert_eq!(repos[0].name, "http://archive.ubuntu.com/ubuntu/");
        assert_eq!(
            repos[0].id,
            "http://archive.ubuntu.com/ubuntu/ noble noble-updates noble-backports main universe restricted multiverse"
        );
        assert_eq!(
            repos[0].url.as_deref(),
            Some("http://archive.ubuntu.com/ubuntu/")
        );
        assert!(repos[0].enabled);
        assert_eq!(repos[0].source, SourceType::Apt);

        assert_eq!(repos[1].name, "http://security.ubuntu.com/ubuntu/");
        assert_eq!(
            repos[1].id,
            "http://security.ubuntu.com/ubuntu/ noble-security main universe restricted multiverse"
        );
    }

    #[test]
    fn test_parse_deb822_sources_empty() {
        assert!(parse_deb822_sources("").is_empty());
    }

    #[test]
    fn test_parse_deb822_sources_disabled() {
        let input = "\
Types: deb
URIs: http://example.com/repo/
Suites: stable
Components: main
Enabled: no
";
        let repos = parse_deb822_sources(input);
        assert_eq!(repos.len(), 1);
        assert!(!repos[0].enabled);
    }

    #[test]
    fn test_parse_deb822_sources_skips_deb_src_only() {
        let input = "\
Types: deb-src
URIs: http://example.com/repo/
Suites: stable
Components: main
";
        let repos = parse_deb822_sources(input);
        assert!(repos.is_empty());
    }

    #[test]
    fn test_parse_deb822_sources_with_comments() {
        let input = "\
## Ubuntu distribution repository
Types: deb
URIs: http://archive.ubuntu.com/ubuntu/
Suites: noble
Components: main
";
        let repos = parse_deb822_sources(input);
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].name, "http://archive.ubuntu.com/ubuntu/");
    }

    #[test]
    fn test_parse_deb822_sources_case_insensitive() {
        let input = "\
types: deb
uris: http://example.com/repo/
suites: stable
components: main
";
        let repos = parse_deb822_sources(input);
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].name, "http://example.com/repo/");
        assert_eq!(repos[0].id, "http://example.com/repo/ stable main");
    }

    #[test]
    fn test_parse_deb822_sources_multiple_uris() {
        let input = "\
Types: deb
URIs: http://archive.ubuntu.com/ubuntu/ http://mirror.example.com/ubuntu/
Suites: noble
Components: main
";
        let repos = parse_deb822_sources(input);
        assert_eq!(repos.len(), 2);
        assert_eq!(repos[0].name, "http://archive.ubuntu.com/ubuntu/");
        assert_eq!(repos[1].name, "http://mirror.example.com/ubuntu/");
    }

    #[test]
    fn test_parse_deb822_sources_deb_and_deb_src() {
        let input = "\
Types: deb deb-src
URIs: http://archive.ubuntu.com/ubuntu/
Suites: noble
Components: main
";
        let repos = parse_deb822_sources(input);
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].name, "http://archive.ubuntu.com/ubuntu/");
    }

    #[test]
    fn test_parse_deb822_sources_no_suites_no_components() {
        let input = "\
Types: deb
URIs: http://example.com/flat-repo/
";
        let repos = parse_deb822_sources(input);
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].id, "http://example.com/flat-repo/");
    }

    #[test]
    fn test_parse_check_updates_no_arch_bracket() {
        let input = "Inst pkg [1.0] (2.0 Debian:stable)\nConf pkg (2.0 Debian:stable)\n";
        let updates = parse_check_updates_output(input);
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].name, "pkg");
        assert_eq!(updates[0].installed_version, "1.0");
        assert_eq!(updates[0].available_version, "2.0");
        assert!(updates[0].arch.is_none());
    }

    #[test]
    fn test_parse_list_installed_empty_description() {
        let input = "meta-pkg\t1.0\tamd64\t\n";
        let packages = parse_list_installed_output(input);
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name, "meta-pkg");
        assert!(packages[0].description.is_none());
    }

    #[test]
    fn test_parse_list_installed_missing_description() {
        let input = "some-lib\t2.0\tamd64\n";
        let packages = parse_list_installed_output(input);
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name, "some-lib");
        assert!(packages[0].description.is_none());
    }

    #[test]
    fn test_parse_sources_list_skips_deb_src() {
        let input = "deb http://example.com/repo stable main\ndeb-src http://example.com/repo stable main\n";
        let repos = parse_sources_list(input);
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].name, "http://example.com/repo");
    }

    #[test]
    fn test_parse_search_description_with_dash() {
        let input = "vim-runtime - Vi IMproved - Runtime files\n";
        let packages = parse_search_output(input);
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name, "vim-runtime");
        assert_eq!(
            packages[0].description.as_deref(),
            Some("Vi IMproved - Runtime files")
        );
    }
}
