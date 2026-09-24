use std::borrow::Cow;
use std::io::{self, Write};
use std::path::Path;

use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use rust_i18n::t;
use tokio::sync::mpsc;
use unicode_width::UnicodeWidthStr;

use pike_core::manager::PackageManager;
use pike_core::package::{CleanupItem, SourceType};
use pike_core::util::truncate_str;

use crate::ipc::notify_daemon_recheck;

use super::app::{Action, App};
use super::async_ops::{AsyncResult, spawn_check_updates, spawn_list_repos, spawn_search};
use super::types::AddRepoParams;

pub(super) async fn handle_action(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    action: Action,
    manager: &PackageManager,
    tx: &mpsc::UnboundedSender<AsyncResult>,
    active_sources: &[SourceType],
    config_path: &Path,
) -> anyhow::Result<()> {
    match action {
        Action::Quit => {
            app.running = false;
        }
        Action::SearchSubmit(query) => {
            let config = app.config.clone();
            spawn_search(app, tx, active_sources, query, config);
        }
        Action::RefreshUpdates => {
            spawn_check_updates(app, tx, active_sources);
        }
        Action::RefreshInstalled => {
            super::async_ops::spawn_list_installed(app, tx, active_sources);
        }
        Action::InstallPackage(pkg, source) => {
            install_package(terminal, app, manager, &pkg, source).await?;
            app.installed.loaded = false;
            spawn_daemon_recheck();
        }
        Action::RemovePackage(pkg, source) => {
            remove_package(terminal, app, manager, &pkg, source).await?;
            app.installed.loaded = false;
            spawn_daemon_recheck();
        }
        Action::UpdatePackage(pkg, source) => {
            update_package(terminal, manager, &pkg, source).await?;
            spawn_check_updates(app, tx, active_sources);
            spawn_daemon_recheck();
        }
        Action::UpdateAll(pkgs) => {
            update_all(terminal, manager, &pkgs).await?;
            spawn_check_updates(app, tx, active_sources);
            spawn_daemon_recheck();
        }
        Action::RefreshCleanup => {
            super::async_ops::spawn_list_cleanup(app, tx, active_sources);
        }
        Action::Clean(items) => {
            clean(terminal, app, manager, &items).await?;
            app.cleanup.loaded = false;
            app.installed.loaded = false;
            spawn_check_updates(app, tx, active_sources);
            spawn_daemon_recheck();
        }
        Action::RefreshRepos => {
            spawn_list_repos(app, tx, active_sources);
        }
        Action::ToggleRepo(id, enabled, source) => {
            toggle_repo(terminal, app, manager, &id, enabled, source).await?;
            spawn_list_repos(app, tx, active_sources);
        }
        Action::AddRepo(params) => {
            add_repo(terminal, app, manager, params).await?;
            spawn_list_repos(app, tx, active_sources);
        }
        Action::DeleteRepo(id, source) => {
            delete_repo(terminal, app, manager, &id, source).await?;
            spawn_list_repos(app, tx, active_sources);
        }
        Action::OpenUrl(url) => {
            let _ = std::process::Command::new("xdg-open")
                .arg(&url)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        }
        Action::SaveSettings => {
            std::fs::write(config_path, app.config.to_toml_commented())?;
            crate::ipc::try_daemon_request(&crate::ipc::DaemonRequest::ReloadConfig);
            app.set_status(t!("tui.status.settings-saved"));
        }
    }
    Ok(())
}

fn spawn_daemon_recheck() {
    std::thread::spawn(notify_daemon_recheck);
}

async fn install_package(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    manager: &PackageManager,
    pkg: &str,
    source: Option<SourceType>,
) -> anyhow::Result<()> {
    let label = t!("tui.action.installing", pkg = pkg);
    let ok = run_interactive(terminal, &label, &[pkg.to_string()], || async {
        let result = manager.install(pkg, source).await;
        match &result {
            Ok(src) => {
                let msg = t!("tui.action.installed-via", pkg = pkg, source = src);
                eprintln!("\n  {msg}");
            }
            Err(e) => {
                let msg = t!("tui.action.error", err = e);
                eprintln!("\n  {msg}");
            }
        }
        result.map(|_| ()).map_err(Into::into)
    })
    .await?;
    if ok {
        if let Some(st) = source {
            app.mark_installed(pkg, st);
        }
        app.set_status(t!("tui.status.installed-pkg", pkg = pkg));
    }
    Ok(())
}

async fn remove_package(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    manager: &PackageManager,
    pkg: &str,
    source: Option<SourceType>,
) -> anyhow::Result<()> {
    let label = t!("tui.action.removing", pkg = pkg);
    let ok = run_interactive(terminal, &label, &[pkg.to_string()], || async {
        let result = manager.remove(pkg, source, false).await;
        match &result {
            Ok(src) => {
                let msg = t!("tui.action.removed-via", pkg = pkg, source = src);
                eprintln!("\n  {msg}");
            }
            Err(e) => {
                let msg = t!("tui.action.error", err = e);
                eprintln!("\n  {msg}");
            }
        }
        result.map(|_| ()).map_err(Into::into)
    })
    .await?;
    if ok {
        if let Some(st) = source {
            app.mark_removed(pkg, st);
        }
        app.set_status(t!("tui.status.removed-pkg", pkg = pkg));
    }
    Ok(())
}

async fn update_package(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    manager: &PackageManager,
    pkg: &str,
    source: SourceType,
) -> anyhow::Result<()> {
    let label = t!("tui.action.updating", pkg = pkg);
    run_interactive(terminal, &label, &[pkg.to_string()], || async {
        if let Err(e) = manager.update_source(pkg, source).await {
            let msg = t!("tui.action.error", err = e);
            eprintln!("\n  {msg}");
        } else {
            let msg = t!("tui.action.updated", pkg = pkg);
            eprintln!("\n  {msg}");
        }
        Ok(())
    })
    .await?;
    Ok(())
}

async fn update_all(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    manager: &PackageManager,
    pkgs: &[(String, SourceType)],
) -> anyhow::Result<()> {
    let items: Vec<String> = pkgs.iter().map(|(name, _)| name.clone()).collect();
    let mut sources: Vec<SourceType> = pkgs.iter().map(|(_, s)| *s).collect();
    sources.sort();
    sources.dedup();

    let label = t!("tui.action.updating-all");
    run_interactive(terminal, &label, &items, || async {
        for source in &sources {
            if let Err(e) = manager.update_all_source(*source).await {
                let msg = t!("tui.action.error", err = e);
                eprintln!("  [{source}] {msg}");
            }
        }
        let msg = t!("tui.action.update-complete");
        eprintln!("\n  {msg}");
        Ok(())
    })
    .await?;
    Ok(())
}

async fn clean(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    manager: &PackageManager,
    items: &[CleanupItem],
) -> anyhow::Result<()> {
    let label = t!("tui.action.cleaning");
    let summary = crate::format::items_summary(items);
    let ok = run_interactive(terminal, &label, &[summary], || async {
        let mut failed = false;
        for (st, result) in manager.clean(items).await {
            if let Err(e) = result {
                let msg = t!("tui.action.error", err = e);
                eprintln!("  [{st}] {msg}");
                failed = true;
            }
        }
        if failed {
            let msg = t!("tui.status.cleanup-error");
            eprintln!("\n  {msg}");
            anyhow::bail!("clean failed");
        }
        let msg = t!("tui.status.cleanup-complete");
        eprintln!("\n  {msg}");
        Ok(())
    })
    .await?;
    app.last_clean_ok = Some(ok);
    Ok(())
}

async fn toggle_repo(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    manager: &PackageManager,
    id: &str,
    enabled: bool,
    source: SourceType,
) -> anyhow::Result<()> {
    let label = if enabled {
        t!("tui.action.enabling", id = id)
    } else {
        t!("tui.action.disabling", id = id)
    };
    let ok = run_interactive(terminal, &label, &[id.to_string()], || async {
        let result = manager.set_repo_enabled(id, enabled, Some(source)).await;
        match &result {
            Ok(()) => {
                let done = if enabled {
                    t!("tui.status.enabled", id = id)
                } else {
                    t!("tui.status.disabled", id = id)
                };
                eprintln!("\n  {done}");
            }
            Err(e) => {
                let msg = t!("tui.action.error", err = e);
                eprintln!("\n  {msg}");
            }
        }
        result.map_err(Into::into)
    })
    .await?;
    if ok {
        let status = if enabled {
            t!("tui.status.enabled", id = id)
        } else {
            t!("tui.status.disabled", id = id)
        };
        app.set_status(status);
    }
    Ok(())
}

async fn add_repo(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    manager: &PackageManager,
    params: AddRepoParams,
) -> anyhow::Result<()> {
    let label = t!("tui.action.adding-repo", url = &params.url);
    let items: Vec<String> = if params.name.is_empty() {
        vec![params.url.clone()]
    } else {
        vec![format!("{} ({})", params.name, params.url)]
    };
    let ok = run_interactive(terminal, &label, &items, || async {
        let result = manager
            .add_repo(
                params.method,
                &params.repo_id,
                &params.name,
                &params.url,
                params.source,
                params.gpgcheck,
            )
            .await;
        match &result {
            Ok(()) => {
                let msg = t!("tui.action.added-repo");
                eprintln!("\n  {msg}");
            }
            Err(e) => {
                let msg = t!("tui.action.error", err = e);
                eprintln!("\n  {msg}");
            }
        }
        result.map_err(Into::into)
    })
    .await?;
    if ok {
        app.set_status(t!("tui.status.repo-added"));
    }
    Ok(())
}

async fn delete_repo(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    manager: &PackageManager,
    id: &str,
    source: SourceType,
) -> anyhow::Result<()> {
    let label = t!("tui.action.removing-repo", id = id);
    let ok = run_interactive(terminal, &label, &[id.to_string()], || async {
        let result = manager.remove_repo(id, Some(source)).await;
        match &result {
            Ok(()) => {
                let msg = t!("tui.action.removed", id = id);
                eprintln!("\n  {msg}");
            }
            Err(e) => {
                let msg = t!("tui.action.error", err = e);
                eprintln!("\n  {msg}");
            }
        }
        result.map_err(Into::into)
    })
    .await?;
    if ok {
        app.set_status(t!("tui.status.removed-repo", id = id));
    }
    Ok(())
}

fn suspend_terminal(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    action_label: &str,
    items: &[String],
) -> anyhow::Result<usize> {
    super::restore_terminal(terminal)?;

    let (width, height) = crossterm::terminal::size().unwrap_or((80, 24));
    let width = width as usize;

    eprint!("\x1b[2J\x1b[H");

    let action_label = truncate_str(action_label, width.saturating_sub(4));
    let pike_pad = width.saturating_sub(4) / 2;
    let label_pad = width.saturating_sub(action_label.width()) / 2;
    let sep = "─".repeat(width.saturating_sub(4));
    let sep_pad = 2;
    eprintln!();
    eprintln!("{:\u{0020}>pike_pad$}\x1b[1mpike\x1b[0m", "");
    eprintln!("{:\u{0020}>label_pad$}\x1b[2m{action_label}\x1b[0m", "");

    let max_lines = (height as usize / 4).min(4);
    let lines = header_lines(items, width.saturating_sub(4), max_lines);
    for line in &lines {
        let line_pad = width.saturating_sub(line.width()) / 2;
        eprintln!("{:\u{0020}>line_pad$}{line}", "");
    }

    eprintln!("{:\u{0020}>sep_pad$}\x1b[2m{sep}\x1b[0m", "");
    eprintln!();

    let scroll_start = 6 + lines.len();
    eprint!("\x1b[{scroll_start};{height}r\x1b[{scroll_start};1H");
    let _ = io::stderr().flush();

    Ok(width)
}

fn header_lines(items: &[String], width: usize, max_lines: usize) -> Vec<String> {
    let max_lines = max_lines.max(1);
    let mut groups: Vec<Vec<Cow<'_, str>>> = Vec::new();
    let mut used = 0;
    for item in items.iter().map(|item| truncate_str(item, width)) {
        let item_width = item.width();
        if let Some(last) = groups.last_mut()
            && used + 2 + item_width <= width
        {
            last.push(item);
            used += 2 + item_width;
        } else {
            groups.push(vec![item]);
            used = item_width;
        }
    }
    if groups.len() <= max_lines {
        return groups.iter().map(|g| g.join(", ")).collect();
    }
    let kept = &groups[..max_lines - 1];
    let shown: usize = kept.iter().map(Vec::len).sum();
    let mut lines: Vec<String> = kept.iter().map(|g| g.join(", ")).collect();
    lines.push(t!("tui.confirm.more", count = items.len() - shown).to_string());
    lines
}

fn resume_terminal(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    width: usize,
) -> anyhow::Result<()> {
    let prompt = t!("tui.action.return-prompt");
    let prompt_pad = width.saturating_sub(prompt.width()) / 2;
    eprint!("\n{:\u{0020}>prompt_pad$}{prompt}", "");
    let _ = io::stderr().flush();
    let mut buf = String::new();
    let _ = io::stdin().read_line(&mut buf);

    eprint!("\x1b[r");
    let _ = io::stderr().flush();

    *terminal = super::setup_terminal()?;
    Ok(())
}

async fn run_interactive<F, Fut>(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    action_label: &str,
    items: &[String],
    f: F,
) -> anyhow::Result<bool>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    let width = suspend_terminal(terminal, action_label, items)?;

    tracing::info!("action: {action_label}");

    let drain = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .ok()
        .map(|mut sigint| tokio::spawn(async move { while sigint.recv().await.is_some() {} }));

    let success = f().await.is_ok();

    if let Some(d) = drain {
        d.abort();
    }

    tracing::info!("action done: {action_label}, success={success}");

    resume_terminal(terminal, width)?;

    Ok(success)
}

#[cfg(test)]
mod tests {
    use super::header_lines;

    fn items(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("pkg{i:02}")).collect()
    }

    #[test]
    fn test_header_lines_wraps_on_item_boundaries() {
        let lines = header_lines(&items(5), 20, 4);
        assert_eq!(lines, vec!["pkg00, pkg01, pkg02", "pkg03, pkg04"]);
    }

    #[test]
    fn test_header_lines_empty() {
        assert!(header_lines(&[], 20, 4).is_empty());
    }

    #[test]
    fn test_header_lines_caps_with_more() {
        let lines = header_lines(&items(20), 20, 3);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "pkg00, pkg01, pkg02");
        assert_eq!(lines[1], "pkg03, pkg04, pkg05");
        assert!(lines[2].ends_with("14 more"), "{}", lines[2]);
    }

    #[test]
    fn test_header_lines_single_line_budget() {
        assert_eq!(header_lines(&items(3), 20, 1), vec!["pkg00, pkg01, pkg02"]);
        let lines = header_lines(&items(4), 20, 1);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].ends_with("4 more"), "{}", lines[0]);
        assert_eq!(header_lines(&items(4), 20, 0), lines);
    }

    #[test]
    fn test_header_lines_item_wider_than_width() {
        let long = vec!["a-very-long-package-name".to_string(), "b".to_string()];
        assert_eq!(header_lines(&long, 10, 4), vec!["a-very-lo…", "b"]);
    }
}
