use std::collections::BTreeMap;
use std::io::{BufRead, Write};

use owo_colors::OwoColorize;
use pike_core::config::IconStyle;
use pike_core::manager::PackageManager;
use pike_core::package::{PackageUpdate, RepoMethod, SourceType, StatusSummary};
use pike_core::util::truncate_str;
use rust_i18n::t;

use crate::RepoCommands;
use crate::ipc::{self, DaemonRequest, DaemonResponse, notify_daemon_recheck, try_daemon_request};

const WAYBAR_MAX_PER_SOURCE: usize = 3;

fn parse_repo_method(s: &str) -> anyhow::Result<RepoMethod> {
    match s {
        "repofile" | "repo-file" | "repo" => Ok(RepoMethod::RepoFile),
        "copr" => Ok(RepoMethod::Copr),
        "baseurl" | "base-url" => Ok(RepoMethod::BaseUrl),
        "rpm" | "rpm-package" => Ok(RepoMethod::RpmPackage),
        "remote" | "remote-add" => Ok(RepoMethod::RemoteAdd),
        other => anyhow::bail!(
            "unknown method '{}', expected: repofile, copr, baseurl, rpm, remote",
            other
        ),
    }
}

fn parse_source_filter(s: Option<&str>) -> anyhow::Result<Option<SourceType>> {
    match s {
        Some(name) => Ok(Some(
            name.parse::<SourceType>().map_err(|e| anyhow::anyhow!(e))?,
        )),
        None => Ok(None),
    }
}

fn print_json<T: serde::Serialize>(data: &T) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(data)?);
    Ok(())
}

pub async fn search(
    manager: &PackageManager,
    query: &str,
    source: Option<&str>,
    json: bool,
) -> anyhow::Result<()> {
    let source_filter = parse_source_filter(source)?;
    let packages = manager.search(query, source_filter).await?;

    if json {
        return print_json(&packages);
    }

    if packages.is_empty() {
        let msg = t!("cli.search-no-results", query = query);
        eprintln!("  {msg}");
        return Ok(());
    }

    print_packages_table(&packages);
    let msg = t!("cli.packages-found", count = packages.len());
    println!("\n  {msg}");

    Ok(())
}

pub async fn install(
    manager: &PackageManager,
    package: &str,
    source: Option<&str>,
) -> anyhow::Result<()> {
    let source_filter = parse_source_filter(source)?;
    let msg = t!("cli.installing", pkg = package);
    eprintln!("  [{}] {msg}", "pike".cyan());
    let src_name = manager.install(package, source_filter).await?;
    let msg = t!("cli.installed", pkg = package);
    eprintln!("  [{}] {msg}", src_name.cyan());
    notify_daemon_recheck();
    Ok(())
}

pub async fn remove(
    manager: &PackageManager,
    package: &str,
    source: Option<&str>,
    purge: bool,
) -> anyhow::Result<()> {
    let source_filter = parse_source_filter(source)?;
    let action_msg = if purge {
        t!("cli.purging", pkg = package)
    } else {
        t!("cli.removing", pkg = package)
    };
    eprintln!("  [{}] {action_msg}", "pike".cyan());
    let src_name = manager.remove(package, source_filter, purge).await?;
    let done_msg = if purge {
        t!("cli.purged", pkg = package)
    } else {
        t!("cli.removed", pkg = package)
    };
    eprintln!("  [{}] {done_msg}", src_name.cyan());
    notify_daemon_recheck();
    Ok(())
}

pub async fn autoremove(manager: &PackageManager) -> anyhow::Result<()> {
    let action_msg = t!("cli.autoremove-source");
    let mut had_error = false;
    for st in &manager.active_source_types() {
        let name = st.display_name();
        eprintln!("  [{}] {action_msg}", name.cyan());
        if let Err(e) = manager.autoremove_source(*st).await {
            eprintln!("  [{}] {}", name.red(), e);
            had_error = true;
        }
    }
    eprintln!();
    if !had_error {
        let done = t!("cli.cleanup-complete");
        eprintln!("  {}", done.green());
    }
    notify_daemon_recheck();
    Ok(())
}

pub async fn update(manager: &PackageManager, package: Option<&str>) -> anyhow::Result<()> {
    match package {
        Some(pkg) => {
            let msg = t!("cli.updating-pkg", pkg = pkg);
            eprintln!("  [{}] {msg}", "pike".cyan());
            manager.update_package(pkg).await?;
            notify_daemon_recheck();
        }
        None => {
            let msg = t!("cli.updating-source");
            let mut had_error = false;
            for st in &manager.active_source_types() {
                let name = st.display_name();
                eprintln!("  [{}] {msg}", name.cyan());
                if let Err(e) = manager.update_all_source(*st).await {
                    eprintln!("  [{}] {}", name.red(), e);
                    had_error = true;
                }
            }
            eprintln!();
            if !had_error {
                let done = t!("cli.all-sources-updated");
                eprintln!("  {}", done.green());
            }
            notify_daemon_recheck();
        }
    }
    Ok(())
}

pub async fn check(
    manager: &PackageManager,
    json: bool,
    notify: bool,
    notify_always: bool,
    waybar: bool,
    style: IconStyle,
) -> anyhow::Result<()> {
    if let Some(resp) = try_daemon_request(&DaemonRequest::Check {
        notify,
        notify_always,
    }) {
        if let Some(status) = resp.status {
            return display_check_results(&status, waybar, json, style);
        } else if let Some(err) = resp.error {
            tracing::warn!("daemon check failed: {err}");
        }
    }

    if !json && !waybar {
        let msg = t!("cli.checking");
        for name in &manager.active_source_names() {
            eprintln!("  [{}] {msg}", name.cyan());
        }
        eprintln!();
    }

    let updates = manager.check_updates().await?;
    let status = StatusSummary::from_fresh_check(updates);

    if (status.total > 0 && notify) || (status.total == 0 && notify_always) {
        send_notification(&status);
    }

    display_check_results(&status, waybar, json, style)
}

pub async fn list(manager: &PackageManager, updates_only: bool, json: bool) -> anyhow::Result<()> {
    if updates_only {
        let status = manager.get_cached_status()?;
        if json {
            return print_json(&status.updates);
        }

        if status.updates.is_empty() {
            let msg = t!("cli.no-cached-updates");
            println!("  {}", msg.dimmed());
            return Ok(());
        }

        print_updates_table(&status.updates);
        return Ok(());
    }

    let packages = manager.list_installed(None).await?;

    if json {
        return print_json(&packages);
    }

    if packages.is_empty() {
        let msg = t!("cli.no-installed");
        eprintln!("  {msg}");
        return Ok(());
    }

    print_packages_table(&packages);
    let msg = t!("cli.packages-found", count = packages.len());
    println!("\n  {msg}");

    Ok(())
}

pub fn status(
    manager: &PackageManager,
    waybar: bool,
    notify: bool,
    notify_always: bool,
    json: bool,
    style: IconStyle,
) -> anyhow::Result<()> {
    let status = if let Some(resp) = try_daemon_request(&DaemonRequest::Status)
        && let Some(status) = resp.status
    {
        status
    } else {
        manager.get_cached_status()?
    };

    display_status(&status, waybar, notify, notify_always, json, style)
}

fn display_status(
    status: &StatusSummary,
    waybar: bool,
    notify: bool,
    notify_always: bool,
    json: bool,
    style: IconStyle,
) -> anyhow::Result<()> {
    if (status.total > 0 && notify) || (status.total == 0 && notify_always) {
        send_notification(status);
    }

    if waybar {
        print_waybar_status(status, style);
        return Ok(());
    }

    if json {
        let mut json_val = serde_json::json!({
            "total": status.total,
            "updates": status.updates,
        });
        for &st in SourceType::ALL {
            json_val[st.display_name()] = serde_json::json!(status.count(st));
        }
        return print_json(&json_val);
    }

    if status.total == 0 {
        let msg = t!("cli.all-up-to-date-short");
        println!("  {}", msg.green());
    } else {
        let summary = format_update_counts(status);
        let msg = t!(
            "cli.updates-status",
            count = status.total,
            summary = &summary
        );
        println!("  {msg}");
    }

    Ok(())
}

pub async fn repo(
    manager: &PackageManager,
    command: RepoCommands,
    json: bool,
) -> anyhow::Result<()> {
    match command {
        RepoCommands::List { source } => {
            let source_filter = parse_source_filter(source.as_deref())?;
            let repos = manager.list_repos(source_filter).await?;
            cmd_repo_list(&repos, json)?;
        }
        RepoCommands::Enable { repo_id, source } => {
            let source_filter = parse_source_filter(source.as_deref())?;
            let msg = t!("cli.enabling", id = &repo_id);
            eprintln!("  [{}] {msg}", "pike".cyan());
            manager
                .set_repo_enabled(&repo_id, true, source_filter)
                .await?;
            let msg = t!("cli.enabled", id = &repo_id);
            eprintln!("  {}", msg.green());
        }
        RepoCommands::Disable { repo_id, source } => {
            let source_filter = parse_source_filter(source.as_deref())?;
            let msg = t!("cli.disabling", id = &repo_id);
            eprintln!("  [{}] {msg}", "pike".cyan());
            manager
                .set_repo_enabled(&repo_id, false, source_filter)
                .await?;
            let msg = t!("cli.disabled", id = &repo_id);
            eprintln!("  {}", msg.dimmed());
        }
        RepoCommands::Add {
            repo_id,
            name,
            url,
            source,
            method,
        } => {
            let source_type: SourceType = source.parse().map_err(|e: String| anyhow::anyhow!(e))?;
            let repo_method = match method.as_deref() {
                Some(m) => parse_repo_method(m)?,
                None => RepoMethod::methods_for(source_type)[0],
            };
            let repo_id = repo_id.unwrap_or_default();
            let msg = t!("cli.adding-repo", name = &name, url = &url);
            eprintln!("  [{}] {msg}", source_type.to_string().cyan());
            manager
                .add_repo(repo_method, &repo_id, &name, &url, source_type, true)
                .await?;
            let msg = t!("cli.added-repo", name = &name);
            eprintln!("  {}", msg.green());
        }
        RepoCommands::Remove { repo_id, source } => {
            let source_filter = parse_source_filter(source.as_deref())?;
            let msg = t!("cli.removing-repo", id = &repo_id);
            eprintln!("  [{}] {msg}", "pike".cyan());
            manager.remove_repo(&repo_id, source_filter).await?;
            let msg = t!("cli.removed-repo", id = &repo_id);
            eprintln!("  {msg}");
        }
    }
    Ok(())
}

fn display_check_results(
    status: &StatusSummary,
    waybar: bool,
    json: bool,
    style: IconStyle,
) -> anyhow::Result<()> {
    if waybar {
        print_waybar_status(status, style);
        return Ok(());
    }
    if json {
        return print_json(&status.updates);
    }
    if status.total == 0 {
        let msg = t!("cli.all-up-to-date");
        println!("  {}", msg.green());
        return Ok(());
    }
    print_updates_table(&status.updates);
    let summary = format_update_counts(status);
    let msg = t!(
        "cli.updates-available",
        count = status.total,
        summary = &summary
    );
    println!("\n  {msg}");
    Ok(())
}

fn print_packages_table(packages: &[pike_core::package::Package]) {
    let h_source = t!("header.source-upper");
    let h_package = t!("header.package");
    let h_arch = t!("header.arch-upper");
    let h_version = t!("header.version-upper");
    let h_desc = t!("header.description-upper");
    println!(
        "  {:<10} {:<30} {:<10} {:<12} {}",
        h_source.bold(),
        h_package.bold(),
        h_arch.bold(),
        h_version.bold(),
        h_desc.bold(),
    );
    for pkg in packages {
        let source_str = pkg.source.to_string();
        let arch = pkg.arch.as_deref().unwrap_or("-");
        let desc = pkg.description.as_deref().unwrap_or("");
        let desc_truncated = truncate_str(desc, 50);
        println!(
            "  {:<10} {:<30} {:<10} {:<12} {}",
            source_str.cyan(),
            pkg.name,
            arch.dimmed(),
            pkg.version.dimmed(),
            desc_truncated,
        );
    }
}

fn print_updates_table(updates: &[PackageUpdate]) {
    let h_source = t!("header.source-upper");
    let h_package = t!("header.package");
    let h_arch = t!("header.arch-upper");
    let h_installed = t!("header.installed-upper");
    let h_available = t!("header.available-upper");
    println!(
        "  {:<10} {:<25} {:<10} {:<15} {}",
        h_source.bold(),
        h_package.bold(),
        h_arch.bold(),
        h_installed.bold(),
        h_available.bold(),
    );
    for u in updates {
        let source_str = u.source.to_string();
        let arch = u.arch.as_deref().unwrap_or("-");
        let installed = if u.installed_version.is_empty() {
            "-"
        } else {
            &u.installed_version
        };
        println!(
            "  {:<10} {:<25} {:<10} {:<15} {}",
            source_str.cyan(),
            u.name,
            arch.dimmed(),
            installed.dimmed(),
            u.available_version.green(),
        );
    }
}

fn cmd_repo_list(repos: &[pike_core::package::Repository], json: bool) -> anyhow::Result<()> {
    if json {
        return print_json(&repos);
    }

    if repos.is_empty() {
        let msg = t!("cli.no-repos");
        eprintln!("  {msg}");
        return Ok(());
    }

    let h_source = t!("header.source-upper");
    let h_repo_id = t!("header.repo-id-upper");
    let h_name = t!("header.name-upper");
    let h_status = t!("header.status-upper");
    println!(
        "  {:<10} {:<25} {:<30} {}",
        h_source.bold(),
        h_repo_id.bold(),
        h_name.bold(),
        h_status.bold(),
    );
    for repo in repos {
        let source_str = repo.source.to_string();
        let status = if repo.enabled {
            "●".green().to_string()
        } else {
            "○".red().to_string()
        };
        println!(
            "  {:<10} {:<25} {:<30} {}",
            source_str.cyan(),
            repo.id,
            truncate_str(&repo.name, 28),
            status,
        );
    }
    let msg = t!("cli.repo-count", count = repos.len());
    println!("\n  {msg}");
    Ok(())
}

pub fn format_grouped_updates(updates: &[PackageUpdate], max_per_source: usize) -> String {
    let mut grouped: BTreeMap<SourceType, Vec<&PackageUpdate>> = BTreeMap::new();
    for u in updates {
        grouped.entry(u.source).or_default().push(u);
    }

    let mut sections: Vec<String> = Vec::new();

    for (source, pkgs) in &grouped {
        let mut lines = Vec::new();
        let count = pkgs.len();
        lines.push(format!(
            " {source} - {count} update{}",
            if count == 1 { "" } else { "s" }
        ));

        for u in pkgs.iter().take(max_per_source) {
            let installed = if u.installed_version.is_empty() {
                "?"
            } else {
                &u.installed_version
            };
            lines.push(format!(
                "  {} {} \u{2192} {}",
                u.name, installed, u.available_version
            ));
        }

        if count > max_per_source {
            lines.push(format!("  \u{2026} and {} more", count - max_per_source));
        }

        sections.push(lines.join("\n"));
    }

    sections.join("\n\n")
}

fn waybar_icons(style: IconStyle) -> (&'static str, &'static str, &'static str) {
    match style {
        IconStyle::Nerd => ("\u{f012c}", "\u{f03d7}", "\u{f002a}"),
        IconStyle::Unicode => ("\u{2713}", "\u{2b06}", "\u{26a0}"),
    }
}

pub fn waybar_json(status: &StatusSummary, style: IconStyle) -> String {
    let (up_to_date, has_updates, _) = waybar_icons(style);
    let checked_suffix = format_checked_at(status.checked_at.as_deref());
    if status.total == 0 {
        let base = t!("waybar.tooltip-up-to-date");
        let tooltip = match &checked_suffix {
            Some(line) => format!("{base}\n{line}"),
            None => base.to_string(),
        };
        serde_json::json!({
            "text": up_to_date,
            "tooltip": tooltip,
            "class": "up-to-date",
        })
        .to_string()
    } else {
        let base = format_grouped_updates(&status.updates, WAYBAR_MAX_PER_SOURCE);
        let tooltip = match &checked_suffix {
            Some(line) => format!("{base}\n\n{line}"),
            None => base,
        };
        serde_json::json!({
            "text": format!("{has_updates}  {}", status.total),
            "tooltip": tooltip,
            "class": "has-updates",
        })
        .to_string()
    }
}

fn format_checked_at(checked_at: Option<&str>) -> Option<String> {
    let raw = checked_at?;
    let dt = match chrono::DateTime::parse_from_rfc3339(raw) {
        Ok(dt) => dt,
        Err(e) => {
            tracing::warn!("failed to parse checked_at timestamp '{raw}': {e}");
            return None;
        }
    };
    let local = dt.with_timezone(&chrono::Local);
    Some(t!("waybar.last-checked", time = local.format("%Y-%m-%d %H:%M")).to_string())
}

fn print_waybar_status(status: &StatusSummary, style: IconStyle) {
    println!("{}", waybar_json(status, style));
}

pub(crate) fn send_notification(status: &StatusSummary) {
    let mut cmd = std::process::Command::new("notify-send");
    cmd.args(["-a", "Pike", "-u", "low"]);
    if status.total == 0 {
        cmd.arg("Pike");
        let body = t!("waybar.tooltip-up-to-date");
        cmd.arg(&*body);
    } else {
        let title = t!("cli.notify-title", count = status.total);
        let body = format_grouped_updates(&status.updates, WAYBAR_MAX_PER_SOURCE);
        cmd.arg(&*title).arg(&body);
    }
    if let Err(e) = cmd.spawn() {
        tracing::warn!("failed to send notification: {e}");
    }
}

pub(crate) fn format_update_counts(status: &StatusSummary) -> String {
    let parts: Vec<String> = SourceType::ALL
        .iter()
        .filter_map(|&st| {
            let c = status.count(st);
            if c > 0 {
                Some(format!("{c} {st}"))
            } else {
                None
            }
        })
        .collect();
    if parts.is_empty() {
        return t!("cli.total", count = status.total).to_string();
    }
    parts.join(" \u{00b7} ")
}

pub fn waybar_continuous(style: IconStyle) -> anyhow::Result<()> {
    let (_, _, error_icon) = waybar_icons(style);
    let path = ipc::socket_path();
    let mut stream = match std::os::unix::net::UnixStream::connect(&path) {
        Ok(s) => s,
        Err(e) => {
            tracing::debug!("could not connect to daemon: {e}");
            println!(
                "{}",
                serde_json::json!({
                    "text": error_icon,
                    "tooltip": t!("waybar.tooltip-daemon-not-running").to_string(),
                    "class": "error",
                })
            );
            return Ok(());
        }
    };

    let req = serde_json::to_string(&DaemonRequest::Subscribe)?;
    stream.write_all(req.as_bytes())?;
    stream.write_all(b"\n")?;

    let reader = std::io::BufReader::new(stream);
    let mut stdout = std::io::stdout().lock();

    for line in reader.lines() {
        let line = line?;
        match serde_json::from_str::<DaemonResponse>(&line) {
            Ok(resp) => {
                if let Some(status) = resp.status {
                    let json = waybar_json(&status, style);
                    writeln!(stdout, "{json}")?;
                    stdout.flush()?;
                } else if let Some(err) = resp.error {
                    tracing::warn!("daemon error in subscription: {err}");
                }
            }
            Err(e) => {
                tracing::warn!("failed to parse daemon push: {e}");
            }
        }
    }

    Ok(())
}
