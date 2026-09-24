use async_trait::async_trait;

use crate::cleanup::{
    cache_item, of_kind, old_kernel_items, preview_removal, removal_list, remove_and_clean_cache,
    running_kernel,
};
use crate::error::PikeError;
use crate::package::{
    CleanupItem, CleanupKind, Package, PackageUpdate, RepoMethod, Repository, SourceType,
};
use crate::source::{
    PackageSource, Result, run_captured, run_captured_allow_exit, run_captured_c, run_privileged,
};

const APT_KERNEL_PREFIXES: [&str; 4] = [
    "linux-image-",
    "linux-modules-",
    "linux-headers-",
    "linux-tools-",
];
const APT_CACHE_DIR: &str = "/var/cache/apt/archives";

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

    async fn list_cleanup(
        &self,
        kinds: &[CleanupKind],
        keep_kernels: usize,
    ) -> Result<Vec<CleanupItem>> {
        let mut items = Vec::new();
        if kinds.contains(&CleanupKind::Orphan) {
            let sim = run_captured_c("apt-get", &["-s", "autoremove"], &[]).await?;
            items = parse_autoremove_sim(&sim);
            if !items.is_empty() {
                let mut args = vec!["-W", "--showformat=${binary:Package}\t${Installed-Size}\n"];
                args.extend(items.iter().map(|i| i.name.as_str()));
                let sizes = parse_installed_sizes(
                    &run_captured_allow_exit("dpkg-query", &args, &[1]).await?,
                );
                for item in &mut items {
                    item.size = sizes.get(&item.name).copied();
                }
            }
        }
        if kinds.contains(&CleanupKind::OldKernel) {
            let output = query_linux_packages().await?;
            items.extend(apt_old_kernel_items(
                &parse_linux_packages(&output),
                running_kernel().as_deref(),
                keep_kernels,
            ));
        }
        if kinds.contains(&CleanupKind::Cache) {
            items.extend(cache_item(SourceType::Apt, APT_CACHE_DIR).await);
        }
        Ok(items)
    }

    async fn clean(&self, items: &[CleanupItem]) -> Result<()> {
        let packages = removal_packages(items).await?;
        remove_and_clean_cache(
            items,
            &packages,
            &["apt-get", "remove", "-y"],
            &["apt-get", "clean"],
        )
        .await
    }

    async fn preview_clean(&self, items: &[CleanupItem]) -> Result<Vec<String>> {
        preview_removal(
            SourceType::Apt,
            &removal_packages(items).await?,
            "apt-get",
            &["-s", "remove"],
            &[],
            parse_remove_preview,
        )
        .await
    }
}

async fn removal_packages(items: &[CleanupItem]) -> Result<Vec<String>> {
    let orphans = of_kind(items, CleanupKind::Orphan)
        .map(|i| i.name.clone())
        .collect();
    let versions: Vec<&str> = of_kind(items, CleanupKind::OldKernel)
        .map(|i| i.version.as_str())
        .collect();
    let mut kernel_pkgs = Vec::new();
    if !versions.is_empty() {
        let output = query_linux_packages().await?;
        let linux = parse_linux_packages(&output);
        kernel_pkgs = versions
            .iter()
            .flat_map(|v| apt_kernel_packages_for(&linux, v, &versions))
            .map(|p| p.name.to_string())
            .collect();
    }
    Ok(removal_list(
        orphans,
        kernel_pkgs,
        running_kernel().as_deref(),
    ))
}

async fn query_linux_packages() -> Result<String> {
    run_captured_allow_exit(
        "dpkg-query",
        &[
            "-W",
            "--showformat=${db:Status-Abbrev}\t${Package}\t${Installed-Size}\n",
            "linux-*",
        ],
        &[1],
    )
    .await
}

pub(crate) fn parse_search_output(output: &str) -> Vec<Package> {
    output
        .lines()
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            let (name, description) = line.split_once(" - ")?;
            Some(Package {
                name: name.trim().to_string(),
                display_name: None,
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
                display_name: None,
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

fn remv_lines(output: &str) -> impl Iterator<Item = (&str, &str)> {
    output.lines().filter_map(|line| {
        let rest = line.strip_prefix("Remv ")?;
        let (name, rest) = rest.split_once(' ').unwrap_or((rest, ""));
        let version = rest
            .strip_prefix('[')
            .and_then(|r| r.split_once(']'))
            .map_or("", |(v, _)| v);
        Some((name, version))
    })
}

pub(crate) fn parse_remove_preview(output: &str) -> Vec<String> {
    remv_lines(output)
        .map(|(name, _)| name.to_string())
        .collect()
}

pub(crate) fn parse_autoremove_sim(output: &str) -> Vec<CleanupItem> {
    remv_lines(output)
        .filter(|(name, _)| !is_kernel_package(name))
        .map(|(name, version)| CleanupItem {
            source: SourceType::Apt,
            kind: CleanupKind::Orphan,
            name: name.to_string(),
            version: version.to_string(),
            size: None,
            arch: None,
        })
        .collect()
}

/// A `name:arch` entry is also reachable by its bare name, since apt omits the native arch.
pub(crate) fn parse_installed_sizes(output: &str) -> std::collections::HashMap<String, u64> {
    let mut sizes = std::collections::HashMap::new();
    for line in output.lines() {
        let Some((name, kib)) = line.split_once('\t') else {
            continue;
        };
        let Ok(kib) = kib.trim().parse::<u64>() else {
            continue;
        };
        if let Some((bare, _)) = name.split_once(':') {
            sizes.entry(bare.to_string()).or_insert(kib * 1024);
        }
        sizes.insert(name.to_string(), kib * 1024);
    }
    sizes
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct LinuxPackage<'a> {
    name: &'a str,
    size: Option<u64>,
    held: bool,
}

pub(crate) fn parse_linux_packages(output: &str) -> Vec<LinuxPackage<'_>> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let status = fields.next()?.as_bytes();
            if status.get(1) != Some(&b'i') {
                return None;
            }
            let name = fields.next()?.trim();
            let size = fields
                .next()
                .and_then(|s| s.trim().parse::<u64>().ok())
                .map(|kib| kib * 1024);
            (!name.is_empty()).then_some(LinuxPackage {
                name,
                size,
                held: status.first() == Some(&b'h'),
            })
        })
        .collect()
}

pub(crate) fn parse_apt_kernels(linux: &[LinuxPackage]) -> Vec<(String, Option<u64>, bool)> {
    linux
        .iter()
        .filter_map(|p| {
            let version = p.name.strip_prefix("linux-image-")?;
            if !version.starts_with(|c: char| c.is_ascii_digit())
                || ["-dbg", "-dbgsym", "-unsigned"]
                    .iter()
                    .any(|s| version.ends_with(s))
            {
                return None;
            }
            let pkgs: Vec<&LinuxPackage> =
                apt_kernel_packages_for(linux, version, &[version]).collect();
            let size = pkgs.iter().filter_map(|p| p.size).reduce(|a, b| a + b);
            let held = pkgs.iter().any(|p| p.held);
            Some((version.to_string(), size, held))
        })
        .collect()
}

/// Held kernels count toward `keep` but are never offered: `apt-get -y` refuses to touch them.
pub(crate) fn apt_old_kernel_items(
    linux: &[LinuxPackage],
    running: Option<&str>,
    keep: usize,
) -> Vec<CleanupItem> {
    let kernels = parse_apt_kernels(linux);
    let held: Vec<String> = kernels
        .iter()
        .filter(|(_, _, held)| *held)
        .map(|(v, _, _)| v.clone())
        .collect();
    old_kernel_items(
        kernels.into_iter().map(|(v, size, _)| (v, size)).collect(),
        running,
        keep,
        SourceType::Apt,
        |v| format!("linux-image-{v}"),
    )
    .into_iter()
    .filter(|i| !held.contains(&i.version))
    .collect()
}

/// Headers and tools shared by the kernel's ABI (`linux-[F-]headers-X`, `linux-[F-]tools-X`)
/// are included only when every installed image of that ABI is being removed.
pub(crate) fn apt_kernel_packages_for<'a>(
    linux: &'a [LinuxPackage<'a>],
    version: &str,
    removing: &[&str],
) -> impl Iterator<Item = &'a LinuxPackage<'a>> {
    let full = format!("-{version}");
    let is_image = |x: &str| {
        linux
            .iter()
            .any(|p| p.name.strip_prefix("linux-image-") == Some(x))
    };
    let abi = linux
        .iter()
        .filter_map(|p| shared_abi(p.name))
        .filter(|x| has_abi(version, x) && !is_image(x))
        .min_by_key(|x| x.len());
    let shared_unused = abi.is_some_and(|abi| {
        !linux.iter().any(|p| {
            p.name
                .strip_prefix("linux-image-")
                .is_some_and(|v| has_abi(v, abi) && !removing.contains(&v))
        })
    });
    linux.iter().filter(move |p| {
        (APT_KERNEL_PREFIXES
            .iter()
            .any(|pre| p.name.starts_with(pre))
            && p.name.ends_with(&full))
            || (shared_unused && shared_abi(p.name) == abi)
    })
}

fn is_kernel_package(name: &str) -> bool {
    APT_KERNEL_PREFIXES.iter().any(|p| name.starts_with(p)) || shared_abi(name).is_some()
}

fn shared_abi(name: &str) -> Option<&str> {
    let rest = name.strip_prefix("linux-")?;
    let rest = ["headers-", "tools-"].iter().find_map(|kind| {
        rest.strip_prefix(kind)
            .or_else(|| rest.split_once(&format!("-{kind}")).map(|(_, r)| r))
    })?;
    let abi = rest.split_once("-common").map_or(rest, |(abi, _)| abi);
    abi.starts_with(|c: char| c.is_ascii_digit()).then_some(abi)
}

fn has_abi(version: &str, abi: &str) -> bool {
    version
        .strip_prefix(abi)
        .is_some_and(|rest| rest.starts_with('-'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cleanup::extra_removals;

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

    const APT_AUTOREMOVE_SIM: &str = "NOTE: This is only a simulation!\nReading package lists...\nRemv libfoo1 [1.2-3]\nRemv linux-image-6.8.0-45-generic [6.8.0-45.45]\nRemv linux-modules-6.8.0-45-generic [6.8.0-45.45]\nRemv linux-tools-6.8.0-45 [6.8.0-45.45]\nRemv python3-bar [0.4-1]\nRemv libbaz1:i386 [1.0]\n";

    const APT_SIZES: &str =
        "libfoo1\t1229\npython3-bar\t340\nlibbaz1:i386\t77\nlibqux1:amd64\t10\n";

    const APT_LINUX_PACKAGES: &str = "ii \tlinux-image-6.8.0-45-generic\t14000\nii \tlinux-modules-6.8.0-45-generic\t80000\nii \tlinux-modules-extra-6.8.0-45-generic\t200000\nii \tlinux-headers-6.8.0-45-generic\t3000\nii \tlinux-headers-6.8.0-45\t90000\nii \tlinux-tools-6.8.0-45-generic\t500\nii \tlinux-tools-6.8.0-45\t700\nii \tlinux-tools-common\t20\nhi \tlinux-image-6.8.0-100-generic\t14100\nii \tlinux-image-6.8.0-90-generic\t14050\nii \tlinux-headers-generic\t10\nrc \tlinux-image-6.8.0-30-generic\t13900\nrc \tlinux-modules-6.8.0-30-generic\t1\n";

    const APT_LINUX_TWO_FLAVOURS: &str = "ii \tlinux-image-6.8.0-45-generic\t14000\nii \tlinux-headers-6.8.0-45-generic\t3000\nii \tlinux-image-6.8.0-45-lowlatency\t14000\nii \tlinux-headers-6.8.0-45-lowlatency\t3000\nii \tlinux-headers-6.8.0-45\t90000\n";

    #[test]
    fn test_parse_autoremove_sim_excludes_kernels() {
        let items = parse_autoremove_sim(APT_AUTOREMOVE_SIM);
        let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["libfoo1", "python3-bar", "libbaz1:i386"]);
        assert_eq!(items[0].version, "1.2-3");
        assert_eq!(items[0].kind, CleanupKind::Orphan);
        assert_eq!(items[0].source, SourceType::Apt);
    }

    #[test]
    fn test_parse_installed_sizes_multiarch() {
        let expected: std::collections::HashMap<String, u64> = [
            ("libfoo1", 1229),
            ("python3-bar", 340),
            ("libbaz1:i386", 77),
            ("libbaz1", 77),
            ("libqux1:amd64", 10),
            ("libqux1", 10),
        ]
        .into_iter()
        .map(|(name, kib)| (name.to_string(), kib * 1024))
        .collect();
        assert_eq!(parse_installed_sizes(APT_SIZES), expected);
    }

    fn names<'a>(pkgs: impl Iterator<Item = &'a LinuxPackage<'a>>) -> Vec<&'a str> {
        pkgs.map(|p| p.name).collect()
    }

    #[test]
    fn test_parse_apt_kernels() {
        assert!(parse_apt_kernels(&parse_linux_packages("")).is_empty());
        let linux = parse_linux_packages(APT_LINUX_PACKAGES);
        let k = parse_apt_kernels(&linux);
        let versions: Vec<&str> = k.iter().map(|(v, ..)| v.as_str()).collect();
        assert_eq!(
            versions,
            vec!["6.8.0-45-generic", "6.8.0-100-generic", "6.8.0-90-generic"]
        );
        assert_eq!(
            k[0].1,
            Some((14000 + 80000 + 200000 + 3000 + 90000 + 500 + 700) * 1024)
        );
        assert_eq!(k[1].1, Some(14100 * 1024));
    }

    #[test]
    fn test_parse_apt_kernels_skips_dbg_and_unsigned() {
        let linux = parse_linux_packages(
            "ii \tlinux-image-6.8.0-45-generic\t1\nii \tlinux-image-6.8.0-45-generic-dbg\t1\nii \tlinux-image-6.8.0-45-generic-dbgsym\t1\nii \tlinux-image-6.8.0-45-unsigned\t1\n",
        );
        let k = parse_apt_kernels(&linux);
        assert_eq!(k.len(), 1);
        assert_eq!(k[0].0, "6.8.0-45-generic");
    }

    #[test]
    fn test_parse_linux_packages_status() {
        let linux = parse_linux_packages(
            "hi \tlinux-image-6.8.0-45-generic\t1\nrc \tlinux-image-6.8.0-30-generic\t1\niU \tlinux-image-6.8.0-31-generic\t1\n",
        );
        assert_eq!(
            linux,
            vec![LinuxPackage {
                name: "linux-image-6.8.0-45-generic",
                size: Some(1024),
                held: true,
            }]
        );
    }

    #[test]
    fn test_apt_old_kernel_items_held() {
        let linux = parse_linux_packages(APT_LINUX_PACKAGES);
        let versions = |items: Vec<CleanupItem>| -> Vec<String> {
            items.into_iter().map(|i| i.version).collect()
        };
        assert_eq!(
            versions(apt_old_kernel_items(&linux, Some("7.0.0-1-generic"), 1)),
            vec!["6.8.0-90-generic", "6.8.0-45-generic"]
        );
        let held_old = parse_linux_packages(
            "ii \tlinux-image-6.8.0-100-generic\t1\nhi \tlinux-image-6.8.0-45-generic\t1\nii \tlinux-image-6.8.0-30-generic\t1\n",
        );
        assert_eq!(
            versions(apt_old_kernel_items(
                &held_old,
                Some("6.8.0-100-generic"),
                1
            )),
            vec!["6.8.0-30-generic"]
        );
        let held_headers = parse_linux_packages(
            "ii \tlinux-image-6.8.0-100-generic\t1\nii \tlinux-image-6.8.0-45-generic\t1\nhi \tlinux-headers-6.8.0-45-generic\t1\nii \tlinux-image-6.8.0-30-generic\t1\nhi \tlinux-modules-6.8.0-3-generic\t1\n",
        );
        assert_eq!(
            versions(apt_old_kernel_items(
                &held_headers,
                Some("6.8.0-100-generic"),
                1
            )),
            vec!["6.8.0-30-generic"]
        );
    }

    #[test]
    fn test_apt_kernel_packages_for() {
        let linux = parse_linux_packages(APT_LINUX_PACKAGES);
        assert_eq!(
            names(apt_kernel_packages_for(
                &linux,
                "6.8.0-45-generic",
                &["6.8.0-45-generic"]
            )),
            vec![
                "linux-image-6.8.0-45-generic",
                "linux-modules-6.8.0-45-generic",
                "linux-modules-extra-6.8.0-45-generic",
                "linux-headers-6.8.0-45-generic",
                "linux-headers-6.8.0-45",
                "linux-tools-6.8.0-45-generic",
                "linux-tools-6.8.0-45",
            ]
        );
        assert_eq!(
            apt_kernel_packages_for(&linux, "6.8.0-30-generic", &["6.8.0-30-generic"]).count(),
            0
        );
    }

    #[test]
    fn test_apt_kernel_packages_for_keeps_shared_headers_of_kept_flavour() {
        let linux = parse_linux_packages(APT_LINUX_TWO_FLAVOURS);
        assert_eq!(
            names(apt_kernel_packages_for(
                &linux,
                "6.8.0-45-generic",
                &["6.8.0-45-generic"]
            )),
            vec![
                "linux-image-6.8.0-45-generic",
                "linux-headers-6.8.0-45-generic"
            ]
        );
    }

    #[test]
    fn test_apt_kernel_packages_for_shared_headers_when_all_flavours_removed() {
        let linux = parse_linux_packages(APT_LINUX_TWO_FLAVOURS);
        let removing = ["6.8.0-45-generic", "6.8.0-45-lowlatency"];
        for v in removing {
            assert!(
                names(apt_kernel_packages_for(&linux, v, &removing))
                    .contains(&"linux-headers-6.8.0-45")
            );
        }
    }

    const APT_LINUX_DEBIAN: &str = "ii \tlinux-image-6.1.0-25-amd64\t1\nii \tlinux-headers-6.1.0-25-amd64\t1\nii \tlinux-image-6.1.0-25-cloud-amd64\t1\nii \tlinux-headers-6.1.0-25-cloud-amd64\t1\nii \tlinux-headers-6.1.0-25-common\t1\nii \tlinux-image-6.1.0-26-amd64\t1\n";

    #[test]
    fn test_apt_kernel_packages_for_debian_flavours() {
        let linux = parse_linux_packages(APT_LINUX_DEBIAN);
        assert_eq!(
            names(apt_kernel_packages_for(
                &linux,
                "6.1.0-25-amd64",
                &["6.1.0-25-amd64"]
            )),
            vec!["linux-image-6.1.0-25-amd64", "linux-headers-6.1.0-25-amd64"]
        );
        let removing = ["6.1.0-25-amd64", "6.1.0-25-cloud-amd64"];
        assert_eq!(
            names(apt_kernel_packages_for(
                &linux,
                "6.1.0-25-cloud-amd64",
                &removing
            )),
            vec![
                "linux-image-6.1.0-25-cloud-amd64",
                "linux-headers-6.1.0-25-cloud-amd64",
                "linux-headers-6.1.0-25-common",
            ]
        );
    }

    const APT_LINUX_DEBIAN13: &str = "ii \tlinux-image-6.12.43+deb13-amd64\t1\nii \tlinux-headers-6.12.43+deb13-amd64\t1\nii \tlinux-image-6.12.43+deb13-cloud-amd64\t1\nii \tlinux-headers-6.12.43+deb13-cloud-amd64\t1\nii \tlinux-headers-6.12.43+deb13-common\t1\nii \tlinux-image-6.12.48+deb13-amd64\t1\nii \tlinux-headers-6.12.48+deb13-amd64\t1\nii \tlinux-headers-6.12.48+deb13-common\t1\n";

    #[test]
    fn test_apt_kernel_packages_for_debian13_flavours() {
        let linux = parse_linux_packages(APT_LINUX_DEBIAN13);
        assert_eq!(
            names(apt_kernel_packages_for(
                &linux,
                "6.12.43+deb13-amd64",
                &["6.12.43+deb13-amd64"]
            )),
            vec![
                "linux-image-6.12.43+deb13-amd64",
                "linux-headers-6.12.43+deb13-amd64"
            ]
        );
        let removing = ["6.12.43+deb13-amd64", "6.12.43+deb13-cloud-amd64"];
        assert_eq!(
            names(apt_kernel_packages_for(
                &linux,
                "6.12.43+deb13-cloud-amd64",
                &removing
            )),
            vec![
                "linux-image-6.12.43+deb13-cloud-amd64",
                "linux-headers-6.12.43+deb13-cloud-amd64",
                "linux-headers-6.12.43+deb13-common",
            ]
        );
        assert_eq!(
            names(apt_kernel_packages_for(
                &linux,
                "6.12.48+deb13-amd64",
                &["6.12.48+deb13-amd64"]
            )),
            vec![
                "linux-image-6.12.48+deb13-amd64",
                "linux-headers-6.12.48+deb13-amd64",
                "linux-headers-6.12.48+deb13-common",
            ]
        );
    }

    const APT_LINUX_64K: &str = "ii \tlinux-image-6.8.0-45-generic\t1\nii \tlinux-headers-6.8.0-45-generic\t1\nii \tlinux-tools-6.8.0-45-generic\t1\nii \tlinux-image-6.8.0-45-generic-64k\t1\nii \tlinux-headers-6.8.0-45-generic-64k\t1\nii \tlinux-tools-6.8.0-45-generic-64k\t1\nii \tlinux-headers-6.8.0-45\t1\nii \tlinux-tools-6.8.0-45\t1\n";

    #[test]
    fn test_apt_kernel_packages_for_prefix_flavour() {
        let linux = parse_linux_packages(APT_LINUX_64K);
        assert_eq!(
            names(apt_kernel_packages_for(
                &linux,
                "6.8.0-45-generic-64k",
                &["6.8.0-45-generic-64k"]
            )),
            vec![
                "linux-image-6.8.0-45-generic-64k",
                "linux-headers-6.8.0-45-generic-64k",
                "linux-tools-6.8.0-45-generic-64k",
            ]
        );
        let removing = ["6.8.0-45-generic", "6.8.0-45-generic-64k"];
        assert_eq!(
            names(apt_kernel_packages_for(
                &linux,
                "6.8.0-45-generic-64k",
                &removing
            )),
            vec![
                "linux-image-6.8.0-45-generic-64k",
                "linux-headers-6.8.0-45-generic-64k",
                "linux-tools-6.8.0-45-generic-64k",
                "linux-headers-6.8.0-45",
                "linux-tools-6.8.0-45",
            ]
        );
    }

    const APT_LINUX_AWS: &str = "ii \tlinux-image-6.8.0-1015-aws\t1\nii \tlinux-modules-6.8.0-1015-aws\t1\nii \tlinux-headers-6.8.0-1015-aws\t1\nii \tlinux-tools-6.8.0-1015-aws\t1\nii \tlinux-aws-headers-6.8.0-1015\t1\nii \tlinux-aws-tools-6.8.0-1015\t1\nii \tlinux-image-6.8.0-1016-aws\t1\nii \tlinux-modules-6.8.0-1016-aws\t1\nii \tlinux-headers-6.8.0-1016-aws\t1\nii \tlinux-aws-headers-6.8.0-1016\t1\n";

    #[test]
    fn test_apt_kernel_packages_for_aws_shared() {
        let linux = parse_linux_packages(APT_LINUX_AWS);
        assert_eq!(
            names(apt_kernel_packages_for(
                &linux,
                "6.8.0-1015-aws",
                &["6.8.0-1015-aws"]
            )),
            vec![
                "linux-image-6.8.0-1015-aws",
                "linux-modules-6.8.0-1015-aws",
                "linux-headers-6.8.0-1015-aws",
                "linux-tools-6.8.0-1015-aws",
                "linux-aws-headers-6.8.0-1015",
                "linux-aws-tools-6.8.0-1015",
            ]
        );
        let with_second = format!("{APT_LINUX_AWS}ii \tlinux-image-6.8.0-1015-aws-64k\t1\n");
        let linux = parse_linux_packages(&with_second);
        assert_eq!(
            names(apt_kernel_packages_for(
                &linux,
                "6.8.0-1015-aws",
                &["6.8.0-1015-aws"]
            )),
            vec![
                "linux-image-6.8.0-1015-aws",
                "linux-modules-6.8.0-1015-aws",
                "linux-headers-6.8.0-1015-aws",
                "linux-tools-6.8.0-1015-aws",
            ]
        );
        let k = parse_apt_kernels(&parse_linux_packages(APT_LINUX_AWS));
        assert_eq!(k[0], ("6.8.0-1015-aws".to_string(), Some(6 * 1024), false));
        assert_eq!(k[1], ("6.8.0-1016-aws".to_string(), Some(4 * 1024), false));
    }

    const APT_LINUX_HWE: &str = "ii \tlinux-image-6.8.0-45-generic\t1\nii \tlinux-headers-6.8.0-45-generic\t1\nii \tlinux-image-6.8.0-45-lowlatency\t1\nii \tlinux-hwe-6.8-headers-6.8.0-45\t1\nii \tlinux-hwe-6.8-tools-6.8.0-45\t1\nii \tlinux-hwe-6.8-tools-common\t1\n";

    #[test]
    fn test_apt_kernel_packages_for_hwe_shared() {
        let linux = parse_linux_packages(APT_LINUX_HWE);
        assert_eq!(
            names(apt_kernel_packages_for(
                &linux,
                "6.8.0-45-generic",
                &["6.8.0-45-generic"]
            )),
            vec![
                "linux-image-6.8.0-45-generic",
                "linux-headers-6.8.0-45-generic"
            ]
        );
        let removing = ["6.8.0-45-generic", "6.8.0-45-lowlatency"];
        assert_eq!(
            names(apt_kernel_packages_for(
                &linux,
                "6.8.0-45-lowlatency",
                &removing
            )),
            vec![
                "linux-image-6.8.0-45-lowlatency",
                "linux-hwe-6.8-headers-6.8.0-45",
                "linux-hwe-6.8-tools-6.8.0-45",
            ]
        );
    }

    #[test]
    fn test_parse_autoremove_sim_excludes_shared_kernel_packages() {
        let sim = "Remv linux-aws-headers-6.8.0-1015 [6.8.0-1015.16]\nRemv linux-hwe-6.8-tools-6.8.0-45 [6.8.0-45.45]\nRemv linux-hwe-6.8-tools-common [6.8.0-45.45]\nRemv libfoo1 [1.0]\n";
        let names: Vec<String> = parse_autoremove_sim(sim)
            .into_iter()
            .map(|i| i.name)
            .collect();
        assert_eq!(names, vec!["linux-hwe-6.8-tools-common", "libfoo1"]);
    }

    #[test]
    fn test_parse_remove_preview() {
        let sim = "NOTE: This is only a simulation!\nRemv linux-image-6.8.0-45-generic [6.8.0-45.45]\nRemv linux-generic [6.8.0.45.45]\nRemv libbaz1:i386 [1.0]\n";
        let requested = vec![
            "libbaz1:i386".to_string(),
            "linux-image-6.8.0-45-generic".to_string(),
        ];
        assert_eq!(
            extra_removals(parse_remove_preview(sim), &requested, SourceType::Apt).unwrap(),
            vec!["linux-generic"]
        );
        assert!(parse_remove_preview("").is_empty());
    }
}
