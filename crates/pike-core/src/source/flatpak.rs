use async_trait::async_trait;

use crate::cleanup::{extra_removals, of_kind, with_stderr};
use crate::error::PikeError;
use crate::package::{
    CleanupItem, CleanupKind, Package, PackageUpdate, RepoMethod, Repository, SourceType,
};
use crate::source::{
    PackageSource, Result, parse_installed_versions, run_captured, run_captured_with_input,
    run_interactive, run_privileged,
};
use crate::util::truncate_str;

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

    async fn install_many(&self, packages: &[String]) -> Result<()> {
        let mut app_ids = Vec::with_capacity(packages.len());
        for pkg in packages {
            app_ids.push(self.resolve_app_id(pkg).await?);
        }
        let mut args = vec!["install", "-y"];
        let refs: Vec<&str> = app_ids.iter().map(|s| s.as_str()).collect();
        args.extend_from_slice(&refs);
        run_interactive("flatpak", &args).await
    }

    async fn remove(&self, package: &str, purge: bool) -> Result<()> {
        let app_id = self.resolve_app_id(package).await?;
        if purge {
            run_interactive("flatpak", &["uninstall", "-y", "--delete-data", &app_id]).await
        } else {
            run_interactive("flatpak", &["uninstall", "-y", &app_id]).await
        }
    }

    async fn remove_many(&self, packages: &[String], purge: bool) -> Result<()> {
        let mut app_ids = Vec::with_capacity(packages.len());
        for pkg in packages {
            app_ids.push(self.resolve_app_id(pkg).await?);
        }
        let mut args = vec!["uninstall", "-y"];
        if purge {
            args.push("--delete-data");
        }
        let refs: Vec<&str> = app_ids.iter().map(|s| s.as_str()).collect();
        args.extend_from_slice(&refs);
        run_interactive("flatpak", &args).await
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

    async fn update_many(&self, packages: &[String]) -> Result<()> {
        let mut app_ids = Vec::with_capacity(packages.len());
        for pkg in packages {
            app_ids.push(self.resolve_app_id(pkg).await?);
        }
        let mut args = vec!["update", "-y"];
        let refs: Vec<&str> = app_ids.iter().map(|s| s.as_str()).collect();
        args.extend_from_slice(&refs);
        run_interactive("flatpak", &args).await
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
        self.run_remote_op("remote-modify", id, &[flag, id]).await
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
                run_privileged(&[
                    "flatpak",
                    "remote-add",
                    "--system",
                    "--if-not-exists",
                    name,
                    url,
                ])
                .await
            }
            _ => Err(crate::error::PikeError::Other(format!(
                "flatpak does not support {} method",
                method
            ))),
        }
    }

    async fn remove_repo(&self, id: &str) -> Result<()> {
        self.run_remote_op("remote-delete", id, &[id]).await
    }

    async fn list_cleanup(
        &self,
        kinds: &[CleanupKind],
        _keep_kernels: usize,
    ) -> Result<Vec<CleanupItem>> {
        if !kinds.contains(&CleanupKind::UnusedRuntime) {
            return Ok(Vec::new());
        }
        let arch = default_arch().await;
        let (mut items, user) =
            tokio::try_join!(unused_in("--system", &arch), unused_in("--user", &arch))?;
        items.extend(user);
        Ok(dedupe_runtimes(items))
    }

    async fn clean(&self, items: &[CleanupItem]) -> Result<()> {
        let (system_refs, user_refs, _) = installation_refs(items).await?;

        if !user_refs.is_empty() {
            let mut args = vec!["uninstall", "-y", "--user"];
            args.extend(user_refs.iter().map(String::as_str));
            run_interactive("flatpak", &args).await?;
        }

        if !system_refs.is_empty() {
            let mut args = vec!["flatpak", "uninstall", "-y", "--system"];
            args.extend(system_refs.iter().map(String::as_str));
            run_privileged(&args).await?;
        }
        Ok(())
    }

    async fn preview_clean(&self, items: &[CleanupItem]) -> Result<Vec<String>> {
        let (system_refs, user_refs, arch) = installation_refs(items).await?;
        let (mut extras, user) = tokio::try_join!(
            related_removals("--system", &system_refs, &arch),
            related_removals("--user", &user_refs, &arch)
        )?;
        extras.extend(user);
        extras.sort();
        extras.dedup();
        Ok(extras)
    }
}

async fn installation_refs(items: &[CleanupItem]) -> Result<(Vec<String>, Vec<String>, String)> {
    if of_kind(items, CleanupKind::UnusedRuntime).next().is_none() {
        return Ok((Vec::new(), Vec::new(), String::new()));
    }
    let arch = default_arch().await;
    let (system_unused, user_unused) =
        tokio::try_join!(unused_in("--system", &arch), unused_in("--user", &arch))?;
    let (system_refs, user_refs) = partition_refs(items, &system_unused, &user_unused);
    Ok((system_refs, user_refs, arch))
}

async fn related_removals(
    installation: &str,
    refs: &[String],
    default_arch: &str,
) -> Result<Vec<String>> {
    if refs.is_empty() {
        return Ok(Vec::new());
    }
    let mut args = vec![installation];
    args.extend(refs.iter().map(String::as_str));
    let removed = uninstall_dry_run(&args, default_arch)
        .await?
        .iter()
        .map(flatpak_ref)
        .collect();
    extra_removals(removed, refs, SourceType::Flatpak)
}

async fn default_arch() -> String {
    match run_captured("flatpak", &["--default-arch"]).await {
        Ok(out) if !out.trim().is_empty() => out.trim().to_string(),
        result => {
            tracing::debug!("flatpak --default-arch failed: {:?}", result);
            fallback_arch(std::env::consts::ARCH).to_string()
        }
    }
}

fn fallback_arch(rust_arch: &str) -> &str {
    match rust_arch {
        "x86" => "i386",
        other => other,
    }
}

async fn unused_in(installation: &str, default_arch: &str) -> Result<Vec<CleanupItem>> {
    uninstall_dry_run(&["--unused", installation], default_arch).await
}

async fn uninstall_dry_run(args: &[&str], default_arch: &str) -> Result<Vec<CleanupItem>> {
    let mut full = vec!["uninstall"];
    full.extend_from_slice(args);
    let output = run_captured_with_input("flatpak", &full, "n\nn\nn\nn\n", &[1]).await?;
    let items = parse_unused_output(&output.stdout, default_arch);
    check_unused_output(&output.stdout, &items).map_err(|e| with_stderr(e, &output.stderr))?;
    Ok(items)
}

impl FlatpakSource {
    async fn run_remote_op(&self, subcommand: &str, id: &str, args: &[&str]) -> Result<()> {
        let user_remotes = run_captured(
            "flatpak",
            &["remotes", "--user", "--show-disabled", "--columns=name"],
        )
        .await?;
        if contains_remote(&user_remotes, id) {
            let mut full = vec![subcommand, "--user"];
            full.extend_from_slice(args);
            run_interactive("flatpak", &full).await
        } else {
            let mut full = vec!["flatpak", subcommand, "--system"];
            full.extend_from_slice(args);
            run_privileged(&full).await
        }
    }

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

fn contains_remote(output: &str, id: &str) -> bool {
    output.lines().any(|line| line.trim() == id)
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

fn numbered_rows(output: &str) -> impl Iterator<Item = Vec<&str>> {
    output.lines().filter_map(|line| {
        let fields: Vec<&str> = line.trim_start().split('\t').collect();
        let digits = fields.first()?.strip_suffix('.')?;
        (!digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())).then_some(fields)
    })
}

/// flatpak omits the Arch column when every row has the default arch.
pub(crate) fn parse_unused_output(output: &str, default_arch: &str) -> Vec<CleanupItem> {
    numbered_rows(output)
        .filter_map(|fields| {
            let (name, arch, branch) = match fields[..] {
                [_, _, name, branch, _] => (name, default_arch, branch),
                [_, _, name, arch, branch, _] => (name, arch.trim(), branch),
                _ => return None,
            };
            let name = name.trim();
            if name.is_empty() {
                return None;
            }
            Some(CleanupItem {
                source: SourceType::Flatpak,
                kind: CleanupKind::UnusedRuntime,
                name: name.to_string(),
                version: branch.trim().to_string(),
                size: None,
                arch: Some(arch.to_string()),
            })
        })
        .collect()
}

fn check_unused_output(output: &str, items: &[CleanupItem]) -> Result<()> {
    if items.is_empty()
        && (numbered_rows(output).next().is_some() || !output.contains("Nothing unused"))
    {
        return Err(PikeError::Parse {
            source_name: "flatpak".to_string(),
            detail: format!(
                "unexpected output from flatpak uninstall: {}",
                truncate_str(output.trim(), 200)
            ),
        });
    }
    Ok(())
}

fn runtime_key(item: &CleanupItem) -> (&str, Option<&str>, &str) {
    (&item.name, item.arch.as_deref(), &item.version)
}

fn dedupe_runtimes(mut items: Vec<CleanupItem>) -> Vec<CleanupItem> {
    items.sort_by(|a, b| runtime_key(a).cmp(&runtime_key(b)));
    items.dedup_by(|a, b| runtime_key(a) == runtime_key(b));
    items
}

fn is_listed(installed_unused: &[CleanupItem], item: &CleanupItem) -> bool {
    installed_unused
        .iter()
        .any(|u| runtime_key(u) == runtime_key(item))
}

fn flatpak_ref(item: &CleanupItem) -> String {
    format!(
        "{}/{}/{}",
        item.name,
        item.arch.as_deref().unwrap_or_default(),
        item.version
    )
}

fn partition_refs(
    items: &[CleanupItem],
    system_unused: &[CleanupItem],
    user_unused: &[CleanupItem],
) -> (Vec<String>, Vec<String>) {
    let (mut system, mut user) = (Vec::new(), Vec::new());
    for item in of_kind(items, CleanupKind::UnusedRuntime) {
        let in_system = is_listed(system_unused, item);
        let in_user = is_listed(user_unused, item);
        if in_system {
            system.push(flatpak_ref(item));
        }
        if in_user {
            user.push(flatpak_ref(item));
        }
        if !in_system && !in_user {
            tracing::warn!(
                "flatpak: {} is no longer unused, skipping",
                flatpak_ref(item)
            );
        }
    }
    (system, user)
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

    #[test]
    fn test_contains_remote() {
        let output = "claude-origin\npike-test\n";
        assert!(contains_remote(output, "pike-test"));
        assert!(!contains_remote(output, "flathub"));
        assert!(!contains_remote("", "flathub"));
    }

    const FLATPAK_UNUSED_OUTPUT: &str = "\nThese runtimes in installation 'system' are pinned and won't be removed; see flatpak-pin(1):\n  runtime/org.freedesktop.Sdk/x86_64/25.08\n\n\n 1.\t   \torg.gnome.Platform\t49\tr\n 2.\t   \torg.gnome.Platform.Locale\t49\tr\n 3.\t   \torg.freedesktop.Platform.codecs-extra\t25.08-extra\tr\n\nProceed with these changes to the system installation? [Y/n]: n\n";

    #[test]
    fn test_parse_flatpak_unused() {
        let items = parse_unused_output(FLATPAK_UNUSED_OUTPUT, "x86_64");
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].name, "org.gnome.Platform");
        assert_eq!(items[0].version, "49");
        assert_eq!(items[0].kind, CleanupKind::UnusedRuntime);
        assert_eq!(items[0].source, SourceType::Flatpak);
        assert_eq!(items[2].version, "25.08-extra");
        assert!(items.iter().all(|i| i.arch.as_deref() == Some("x86_64")));
    }

    #[test]
    fn test_parse_flatpak_unused_rejects_bare_dot_index() {
        assert!(parse_unused_output(" .\t   \torg.gnome.Platform\t49\tr\n", "x86_64").is_empty());
    }

    fn runtime_item_arch(name: &str, arch: &str, version: &str) -> CleanupItem {
        CleanupItem {
            source: SourceType::Flatpak,
            kind: CleanupKind::UnusedRuntime,
            name: name.to_string(),
            version: version.to_string(),
            size: None,
            arch: Some(arch.to_string()),
        }
    }

    fn runtime_item(name: &str, version: &str) -> CleanupItem {
        runtime_item_arch(name, "x86_64", version)
    }

    #[test]
    fn test_dedupe_runtimes() {
        let items = vec![
            runtime_item("org.gnome.Platform.Locale", "49"),
            runtime_item("org.gnome.Platform", "49"),
            runtime_item_arch("org.gnome.Platform", "i386", "49"),
            runtime_item("org.gnome.Platform", "49"),
            runtime_item("org.gnome.Platform", "48"),
            runtime_item("org.gnome.Platform.Locale", "49"),
        ];
        assert_eq!(
            dedupe_runtimes(items),
            vec![
                runtime_item_arch("org.gnome.Platform", "i386", "49"),
                runtime_item("org.gnome.Platform", "48"),
                runtime_item("org.gnome.Platform", "49"),
                runtime_item("org.gnome.Platform.Locale", "49"),
            ]
        );
    }

    #[test]
    fn test_partition_refs_item_in_both_lists() {
        let items = vec![runtime_item("org.gnome.Platform", "49")];
        let unused = vec![runtime_item("org.gnome.Platform", "49")];
        let expected = vec!["org.gnome.Platform/x86_64/49".to_string()];
        assert_eq!(
            partition_refs(&items, &unused, &unused),
            (expected.clone(), expected)
        );
    }

    #[test]
    fn test_parse_flatpak_unused_with_arch_column() {
        let output = " 1.\t   \torg.freedesktop.Platform.GL32.default\ti386\t25.08\tr\n 2.\t   \torg.gnome.Platform\tx86_64\t49\tr\n";
        let items = parse_unused_output(output, "aarch64");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].name, "org.freedesktop.Platform.GL32.default");
        assert_eq!(items[0].arch.as_deref(), Some("i386"));
        assert_eq!(items[0].version, "25.08");
        assert_eq!(items[1].arch.as_deref(), Some("x86_64"));
        assert_eq!(items[1].version, "49");
    }

    #[test]
    fn test_flatpak_ref_formatting() {
        assert_eq!(
            flatpak_ref(&runtime_item("org.gnome.Platform", "49")),
            "org.gnome.Platform/x86_64/49"
        );
        assert_eq!(
            flatpak_ref(&runtime_item_arch("org.gnome.Platform", "i386", "49")),
            "org.gnome.Platform/i386/49"
        );
    }

    #[test]
    fn test_fallback_arch() {
        assert_eq!(fallback_arch("x86"), "i386");
        assert_eq!(fallback_arch("x86_64"), "x86_64");
        assert_eq!(fallback_arch("aarch64"), "aarch64");
    }

    #[test]
    fn test_check_unused_output() {
        let nothing = "Nothing unused to uninstall\n";
        assert!(parse_unused_output(nothing, "x86_64").is_empty());
        assert!(check_unused_output(nothing, &[]).is_ok());
        assert!(matches!(
            check_unused_output(
                "Proceed with these changes to the system installation? [Y/n]: n",
                &[]
            ),
            Err(PikeError::Parse { .. })
        ));
        let items = parse_unused_output(FLATPAK_UNUSED_OUTPUT, "x86_64");
        assert!(check_unused_output(FLATPAK_UNUSED_OUTPUT, &items).is_ok());
        assert!(matches!(
            check_unused_output("error: No such installation\n", &[]),
            Err(PikeError::Parse { .. })
        ));
    }

    #[test]
    fn test_parse_flatpak_unused_rejects_unknown_field_count() {
        let seven = " 1.\t   \torg.gnome.Platform\tx86_64\t49\tr\textra\nNothing unused\n";
        let items = parse_unused_output(seven, "x86_64");
        assert!(items.is_empty());
        assert!(matches!(
            check_unused_output(seven, &items),
            Err(PikeError::Parse { .. })
        ));
    }

    #[test]
    fn test_partition_refs_negative_cases() {
        let unused = vec![runtime_item("org.gnome.Platform", "49")];
        let not_listed = vec![runtime_item("org.gnome.Platform", "48")];
        assert_eq!(
            partition_refs(&not_listed, &unused, &unused),
            (vec![], vec![])
        );

        let mut wrong_kind = runtime_item("org.gnome.Platform", "49");
        wrong_kind.kind = CleanupKind::Orphan;
        assert_eq!(
            partition_refs(&[wrong_kind], &unused, &[]),
            (vec![], vec![])
        );

        let other_arch = vec![runtime_item_arch("org.gnome.Platform", "i386", "49")];
        assert_eq!(
            partition_refs(&other_arch, &unused, &unused),
            (vec![], vec![])
        );

        assert_eq!(
            partition_refs(&unused, &[], &unused),
            (vec![], vec!["org.gnome.Platform/x86_64/49".to_string()])
        );
    }
}
