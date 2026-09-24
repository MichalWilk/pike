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
    PackageSource, PendingGpgKey, Result, parse_installed_versions, run_captured,
    run_captured_allow_exit, run_captured_stderr, run_interactive, run_privileged,
};

const DNF_CACHE_DIR: &str = "/var/cache/libdnf5";
const KERNEL_PACKAGES: [&str; 7] = [
    "kernel",
    "kernel-core",
    "kernel-modules",
    "kernel-modules-core",
    "kernel-modules-extra",
    "kernel-modules-internal",
    "kernel-devel",
];

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
        run_privileged(&["dnf5", "upgrade", "-y", "--refresh", package]).await
    }

    async fn update_many(&self, packages: &[String]) -> Result<()> {
        let mut args = vec!["dnf5", "upgrade", "-y", "--refresh"];
        let refs: Vec<&str> = packages.iter().map(|s| s.as_str()).collect();
        args.extend_from_slice(&refs);
        run_privileged(&args).await
    }

    async fn refresh_preflight(&self) -> Result<Vec<PendingGpgKey>> {
        let stderr = run_captured_stderr("dnf5", &["check-upgrade", "-q"]).await?;
        Ok(parse_pending_gpg_keys(&stderr))
    }

    async fn import_keys(&self) -> Result<()> {
        run_interactive("dnf5", &["makecache", "--refresh"]).await
    }

    async fn update_all(&self) -> Result<()> {
        run_privileged(&["dnf5", "upgrade", "-y", "--refresh"]).await
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
            RepoMethod::Copr => run_privileged(&["dnf5", "copr", "enable", "-y", url]).await,
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

    async fn list_cleanup(
        &self,
        kinds: &[CleanupKind],
        keep_kernels: usize,
    ) -> Result<Vec<CleanupItem>> {
        let mut items = Vec::new();
        let orphans = kinds.contains(&CleanupKind::Orphan);
        let kernels = kinds.contains(&CleanupKind::OldKernel);
        if orphans || kernels {
            let kernel_output = query_kernel_packages().await?;
            if orphans {
                let unneeded = run_captured(
                    "dnf5",
                    &[
                        "repoquery",
                        "--unneeded",
                        "--queryformat=%{name}.%{arch}\t%{version}-%{release}\t%{installsize}\n",
                    ],
                )
                .await?;
                items.extend(parse_unneeded_output(
                    &unneeded,
                    &installonly_names(&kernel_output),
                ));
            }
            if kernels {
                items.extend(old_kernel_items(
                    kernel_versions(&parse_kernel_packages(&kernel_output)),
                    running_kernel().as_deref(),
                    keep_kernels,
                    SourceType::Dnf,
                    |_| "kernel".to_string(),
                ));
            }
        }
        if kinds.contains(&CleanupKind::Cache) {
            items.extend(cache_item(SourceType::Dnf, DNF_CACHE_DIR).await);
        }
        Ok(items)
    }

    async fn clean(&self, items: &[CleanupItem]) -> Result<()> {
        let packages = removal_packages(items).await?;
        remove_and_clean_cache(
            items,
            &packages,
            &["dnf5", "remove", "-y"],
            &["dnf5", "clean", "all"],
        )
        .await
    }

    async fn preview_clean(&self, items: &[CleanupItem]) -> Result<Vec<String>> {
        preview_removal(
            SourceType::Dnf,
            &removal_packages(items).await?,
            "dnf5",
            &["remove", "--assumeno"],
            &[1],
            parse_remove_preview,
        )
        .await
    }
}

async fn removal_packages(items: &[CleanupItem]) -> Result<Vec<String>> {
    let orphans = of_kind(items, CleanupKind::Orphan)
        .map(|i| orphan_nevra(&i.name, &i.version))
        .collect();
    let versions: Vec<&str> = of_kind(items, CleanupKind::OldKernel)
        .map(|i| i.version.as_str())
        .collect();
    let mut kernel_pkgs = Vec::new();
    if !versions.is_empty() {
        let output = query_kernel_packages().await?;
        let rows = parse_kernel_packages(&output);
        kernel_pkgs = versions
            .into_iter()
            .flat_map(|v| kernel_packages_for(&rows, v))
            .collect();
    }
    Ok(removal_list(
        orphans,
        kernel_pkgs,
        running_kernel().as_deref(),
    ))
}

async fn query_kernel_packages() -> Result<String> {
    run_captured_allow_exit(
        "rpm",
        &[
            "-q",
            "--whatprovides",
            "installonlypkg(kernel)",
            "installonlypkg(kernel-module)",
            "--queryformat=%{NAME}\t%{VERSION}-%{RELEASE}.%{ARCH}\t%{SIZE}\n",
        ],
        &[1, 2],
    )
    .await
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

/// Kernel packages are never orphans here: they are handled per kernel version.
pub(crate) fn parse_unneeded_output(output: &str, installonly: &[&str]) -> Vec<CleanupItem> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let name = fields.next()?.trim();
            let version = fields.next()?.trim();
            let base = name.rsplit_once('.').map_or(name, |(n, _)| n);
            if name.is_empty() || KERNEL_PACKAGES.contains(&base) || installonly.contains(&base) {
                return None;
            }
            let size = fields.next().and_then(|s| s.trim().parse().ok());
            Some(CleanupItem {
                source: SourceType::Dnf,
                kind: CleanupKind::Orphan,
                name: name.to_string(),
                version: version.to_string(),
                size,
                arch: None,
            })
        })
        .collect()
}

/// Full NEVRA, so an installonly package loses only this version.
pub(crate) fn orphan_nevra(name: &str, version: &str) -> String {
    match name.rsplit_once('.') {
        Some((base, arch)) if !version.is_empty() => format!("{base}-{version}.{arch}"),
        _ => name.to_string(),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct KernelPackage<'a> {
    name: &'a str,
    version: &'a str,
    size: Option<u64>,
}

pub(crate) fn parse_kernel_packages(output: &str) -> Vec<KernelPackage<'_>> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.splitn(3, '\t');
            let name = fields.next()?;
            let version = fields.next()?;
            KERNEL_PACKAGES.contains(&name).then(|| KernelPackage {
                name,
                version,
                size: fields.next().and_then(|s| s.trim().parse().ok()),
            })
        })
        .collect()
}

fn installonly_names(output: &str) -> Vec<&str> {
    output
        .lines()
        .filter_map(|line| line.split_once('\t').map(|(name, _)| name))
        .collect()
}

pub(crate) fn parse_remove_preview(output: &str) -> Vec<String> {
    let mut in_removal = false;
    let mut removed = Vec::new();
    for line in output.lines() {
        if !line.starts_with(' ') {
            let header = line.trim_end();
            if header.ends_with(':') {
                in_removal = matches!(
                    header,
                    "Removing:" | "Removing dependent packages:" | "Removing unused dependencies:"
                );
            }
            continue;
        }
        if !in_removal {
            continue;
        }
        let mut fields = line.split_whitespace();
        let (Some(name), Some(arch), Some(evr)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let vr = evr.split_once(':').map_or(evr, |(_, vr)| vr);
        removed.push(format!("{name}-{vr}.{arch}"));
    }
    removed
}

pub(crate) fn kernel_versions(rows: &[KernelPackage]) -> Vec<(String, Option<u64>)> {
    rows.iter()
        .filter(|r| r.name == "kernel-core")
        .map(|core| {
            let size = rows
                .iter()
                .filter(|r| r.version == core.version)
                .filter_map(|r| r.size)
                .reduce(|a, b| a + b);
            (core.version.to_string(), size)
        })
        .collect()
}

pub(crate) fn kernel_packages_for(rows: &[KernelPackage], version: &str) -> Vec<String> {
    rows.iter()
        .filter(|r| r.version == version)
        .map(|r| format!("{}-{}", r.name, r.version))
        .collect()
}

pub(crate) fn parse_pending_gpg_keys(stderr: &str) -> Vec<PendingGpgKey> {
    let mut keys = Vec::new();
    let mut key_id: Option<String> = None;
    let mut user_id: Option<String> = None;

    for line in stderr.lines() {
        let line = line.trim();
        if let Some(idx) = line.find("Importing OpenPGP key") {
            let rest = &line[idx + "Importing OpenPGP key".len()..];
            key_id = Some(rest.trim().trim_end_matches(':').trim().to_string());
            user_id = None;
        } else if line.starts_with("UserID")
            && let (Some(start), Some(end)) = (line.find('"'), line.rfind('"'))
            && end > start
        {
            user_id = Some(line[start + 1..end].to_string());
        } else if line.starts_with("Fingerprint")
            && let (Some(id), Some((_, fp))) = (key_id.take(), line.split_once(':'))
        {
            keys.push(PendingGpgKey {
                key_id: id,
                user_id: user_id.take().unwrap_or_default(),
                fingerprint: fp.trim().to_string(),
            });
        }
    }

    keys
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cleanup::extra_removals;

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

    const DNF_GPG_PROMPT_STDERR: &str = "Importing OpenPGP key 0xDE226D6F:\n UserID     : \"Terra 44 <security@fyralabs.com>\"\n Fingerprint: AE09157A4DE88B497EA1D5D300CDAB43DE226D6F\n From       : file:///etc/pki/rpm-gpg/RPM-GPG-KEY-terra44\nIs this ok [y/N]: Importing OpenPGP key 0x2FFEB650:\n UserID     : \"Terra 44 - Mesa <security@fyralabs.com>\"\n Fingerprint: BED73E7D401960C590E45D957A3DC3E02FFEB650\n From       : file:///etc/pki/rpm-gpg/RPM-GPG-KEY-terra44-mesa\nIs this ok [y/N]: ";

    #[test]
    fn test_parse_pending_gpg_keys() {
        let keys = parse_pending_gpg_keys(DNF_GPG_PROMPT_STDERR);
        assert_eq!(keys.len(), 2);

        assert_eq!(keys[0].key_id, "0xDE226D6F");
        assert_eq!(keys[0].user_id, "Terra 44 <security@fyralabs.com>");
        assert_eq!(
            keys[0].fingerprint,
            "AE09157A4DE88B497EA1D5D300CDAB43DE226D6F"
        );

        assert_eq!(keys[1].key_id, "0x2FFEB650");
        assert_eq!(keys[1].user_id, "Terra 44 - Mesa <security@fyralabs.com>");
        assert_eq!(
            keys[1].fingerprint,
            "BED73E7D401960C590E45D957A3DC3E02FFEB650"
        );
    }

    #[test]
    fn test_parse_pending_gpg_keys_none() {
        assert!(parse_pending_gpg_keys("").is_empty());
        assert!(parse_pending_gpg_keys(" bash.x86_64  5.2.38-1.fc43  updates\n").is_empty());
    }

    #[test]
    fn test_parse_pending_gpg_keys_single() {
        let stderr = "Importing OpenPGP key 0xABCD1234:\n UserID     : \"Fedora <fedora@example.com>\"\n Fingerprint: 1111222233334444555566667777888899990000\n From       : file:///etc/pki/rpm-gpg/RPM-GPG-KEY-fedora\nIs this ok [y/N]: ";
        let keys = parse_pending_gpg_keys(stderr);
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].key_id, "0xABCD1234");
        assert_eq!(keys[0].user_id, "Fedora <fedora@example.com>");
        assert_eq!(
            keys[0].fingerprint,
            "1111222233334444555566667777888899990000"
        );
    }

    #[test]
    fn test_parse_pending_gpg_keys_missing_userid() {
        let stderr =
            "Importing OpenPGP key 0xDEADBEEF:\n Fingerprint: AAAABBBBCCCCDDDD\nIs this ok [y/N]: ";
        let keys = parse_pending_gpg_keys(stderr);
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].key_id, "0xDEADBEEF");
        assert_eq!(keys[0].user_id, "");
        assert_eq!(keys[0].fingerprint, "AAAABBBBCCCCDDDD");
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

    const DNF_UNNEEDED_OUTPUT: &str = "libfoo.x86_64\t1.2-3.fc44\t1258291\npython3-bar.noarch\t0.4-1.fc44\t348160\nkernel-devel.x86_64\t7.2.5-200.fc44\t70000000\nkernelshark.x86_64\t2.3-1.fc44\t5000\nkernel-core.x86_64\t7.2.5-200.fc44\t106627293\nkernel-debug-core.x86_64\t7.2.5-200.fc44\t90000000\n";

    const RPM_KERNEL_PACKAGES: &str = "kernel-modules-core\t7.2.5-200.fc44.x86_64\t77444095\nkernel-core\t7.2.5-200.fc44.x86_64\t106627293\nkernel-modules\t7.2.5-200.fc44.x86_64\t105470864\nkernel\t7.2.5-200.fc44.x86_64\t0\nkernel-debug-core\t7.2.5-200.fc44.x86_64\t90000000\nkernel-core\t7.2.50-200.fc44.x86_64\t1\nkernel-core\t7.2.6-200.fc44.x86_64\t106655508\nkernel\t7.2.6-200.fc44.x86_64\t0\nkernel-devel\t7.2.5-200.fc44.x86_64\t70000000\nno package provides installonlypkg(kernel-module)\n";

    #[test]
    fn test_parse_unneeded() {
        assert!(parse_unneeded_output("", &[]).is_empty());
        assert!(
            parse_unneeded_output("kernel-core.x86_64\t7.2.5-200.fc44\t106627293\n", &[])
                .is_empty()
        );
        let installonly = installonly_names(RPM_KERNEL_PACKAGES);
        let items = parse_unneeded_output(DNF_UNNEEDED_OUTPUT, &installonly);
        let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["libfoo.x86_64", "python3-bar.noarch", "kernelshark.x86_64"]
        );
        assert_eq!(items[0].version, "1.2-3.fc44");
        assert_eq!(items[0].size, Some(1258291));
        assert_eq!(items[0].kind, CleanupKind::Orphan);
        assert_eq!(items[0].source, SourceType::Dnf);
    }

    #[test]
    fn test_orphan_nevra() {
        assert_eq!(
            orphan_nevra("kernel-devel.x86_64", "7.2.5-200.fc44"),
            "kernel-devel-7.2.5-200.fc44.x86_64"
        );
        assert_eq!(
            orphan_nevra("python3.11.x86_64", "3.11.11-1.fc44"),
            "python3.11-3.11.11-1.fc44.x86_64"
        );
        assert_eq!(orphan_nevra("libfoo.x86_64", ""), "libfoo.x86_64");
    }

    #[test]
    fn test_parse_kernel_packages_skips_variants_and_messages() {
        let rows = parse_kernel_packages(RPM_KERNEL_PACKAGES);
        assert_eq!(rows.len(), 8);
        assert!(rows.iter().all(|r| r.name != "kernel-debug-core"));
        assert!(
            parse_kernel_packages("no package provides installonlypkg(kernel-module)\n").is_empty()
        );
    }

    #[test]
    fn test_kernel_versions_sums_sizes() {
        assert!(kernel_versions(&parse_kernel_packages("")).is_empty());
        let rows = parse_kernel_packages(RPM_KERNEL_PACKAGES);
        let versions = kernel_versions(&rows);
        assert_eq!(versions.len(), 3);
        assert_eq!(
            versions[0],
            (
                "7.2.5-200.fc44.x86_64".to_string(),
                Some(77444095 + 106627293 + 105470864 + 70000000)
            )
        );
    }

    #[test]
    fn test_kernel_packages_for_exact_version() {
        let rows = parse_kernel_packages(RPM_KERNEL_PACKAGES);
        let pkgs = kernel_packages_for(&rows, "7.2.5-200.fc44.x86_64");
        assert_eq!(
            pkgs,
            vec![
                "kernel-modules-core-7.2.5-200.fc44.x86_64",
                "kernel-core-7.2.5-200.fc44.x86_64",
                "kernel-modules-7.2.5-200.fc44.x86_64",
                "kernel-7.2.5-200.fc44.x86_64",
                "kernel-devel-7.2.5-200.fc44.x86_64",
            ]
        );
    }

    const DNF_REMOVE_PREVIEW: &str = "Package              Arch   Version          Repository      Size\nRemoving:\n kernel-core         x86_64 0:7.2.5-200.fc44 updates    101.7 MiB\nRemoving dependent packages:\n kernel              x86_64 0:7.2.5-200.fc44 updates      0.0   B\n kernel-modules      x86_64 0:7.2.5-200.fc44 updates    100.6 MiB\n kernel-modules-core x86_64 0:7.2.5-200.fc44 updates     73.9 MiB\n\nTransaction Summary:\n Removing:           4 packages\n";

    fn preview(output: &str, requested: &[&str]) -> Result<Vec<String>> {
        let requested: Vec<String> = requested.iter().map(|s| s.to_string()).collect();
        extra_removals(parse_remove_preview(output), &requested, SourceType::Dnf)
    }

    #[test]
    fn test_parse_remove_preview() {
        assert_eq!(
            preview(DNF_REMOVE_PREVIEW, &["kernel-core-7.2.5-200.fc44.x86_64"]).unwrap(),
            vec![
                "kernel-7.2.5-200.fc44.x86_64",
                "kernel-modules-7.2.5-200.fc44.x86_64",
                "kernel-modules-core-7.2.5-200.fc44.x86_64",
            ]
        );
        let unused = "Removing:\n libfoo x86_64 2:1.2-3.fc44 fedora 1.0 MiB\nRemoving unused dependencies:\n libbar noarch 0.4-1.fc44 fedora 10.0 KiB\n";
        assert_eq!(
            preview(unused, &["libfoo-1.2-3.fc44.x86_64"]).unwrap(),
            vec!["libbar-0.4-1.fc44.noarch"]
        );
    }

    #[test]
    fn test_parse_remove_preview_missing_requested_is_error() {
        let requested = ["libfoo-1.2-3.fc44.x86_64"];
        let nothing =
            "No packages to remove for argument: libfoo-1.2-3.fc44.x86_64\n\nNothing to do.\n";
        for output in ["", nothing] {
            assert!(matches!(
                preview(output, &requested),
                Err(PikeError::Parse { .. })
            ));
        }
    }
}
