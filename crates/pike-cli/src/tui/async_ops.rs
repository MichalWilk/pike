use rust_i18n::t;
use tokio::sync::mpsc;

use pike_core::config::Config;
use pike_core::package::{Package, PackageUpdate, Repository, SourceType};
use pike_core::source::{PackageSource, create_sources};
use pike_core::util::{filter_and_sort_packages, gather, sort_by_source};

use super::app::App;

pub(super) enum AsyncResult {
    SearchResults(Vec<Package>),
    Updates(Vec<PackageUpdate>),
    Installed(Vec<Package>),
    Repos(Vec<Repository>),
}

pub(super) fn spawn_search(
    app: &mut App,
    tx: &mpsc::UnboundedSender<AsyncResult>,
    active_sources: &[SourceType],
    query: String,
    config: Config,
) {
    app.search.results.loading = true;
    let tx = tx.clone();
    let sources = create_sources(active_sources);
    tokio::spawn(async move {
        let results = search_bg(&sources, &query, &config).await;
        let _ = tx.send(AsyncResult::SearchResults(results));
    });
}

pub(super) fn spawn_check_updates(
    app: &mut App,
    tx: &mpsc::UnboundedSender<AsyncResult>,
    active_sources: &[SourceType],
) {
    app.updates.loading = true;
    app.set_status(t!("tui.status.checking"));
    let tx = tx.clone();
    let sources = create_sources(active_sources);
    tokio::spawn(async move {
        let updates = check_updates_bg(&sources).await;
        let _ = tx.send(AsyncResult::Updates(updates));
    });
}

pub(super) fn spawn_list_installed(
    app: &mut App,
    tx: &mpsc::UnboundedSender<AsyncResult>,
    active_sources: &[SourceType],
) {
    app.installed.loading = true;
    app.set_status(t!("tui.status.loading-installed"));
    let tx = tx.clone();
    let sources = create_sources(active_sources);
    tokio::spawn(async move {
        let packages = list_installed_bg(&sources).await;
        let _ = tx.send(AsyncResult::Installed(packages));
    });
}

pub(super) fn spawn_list_repos(
    app: &mut App,
    tx: &mpsc::UnboundedSender<AsyncResult>,
    active_sources: &[SourceType],
) {
    app.repos.list.loading = true;
    app.set_status(t!("tui.status.loading-repos"));
    let tx = tx.clone();
    let sources = create_sources(active_sources);
    tokio::spawn(async move {
        let repos = list_repos_bg(&sources).await;
        let _ = tx.send(AsyncResult::Repos(repos));
    });
}

async fn search_bg(
    sources: &[Box<dyn PackageSource>],
    query: &str,
    config: &Config,
) -> Vec<Package> {
    let futures: Vec<_> = sources.iter().map(|s| s.search(query)).collect();
    let mut packages = gather(futures, "search").await;

    filter_and_sort_packages(&mut packages, config);
    packages
}

async fn check_updates_bg(sources: &[Box<dyn PackageSource>]) -> Vec<PackageUpdate> {
    let futures: Vec<_> = sources.iter().map(|s| s.check_updates()).collect();
    gather(futures, "check_updates").await
}

async fn list_installed_bg(sources: &[Box<dyn PackageSource>]) -> Vec<Package> {
    let futures: Vec<_> = sources.iter().map(|s| s.list_installed()).collect();
    let mut packages = gather(futures, "list_installed").await;
    sort_by_source(&mut packages, |p| &p.source, |p| &p.name);
    packages
}

async fn list_repos_bg(sources: &[Box<dyn PackageSource>]) -> Vec<Repository> {
    let futures: Vec<_> = sources.iter().map(|s| s.list_repos()).collect();
    let mut repos = gather(futures, "list_repos").await;
    sort_by_source(&mut repos, |r| &r.source, |r| r.id.as_str());
    repos
}
