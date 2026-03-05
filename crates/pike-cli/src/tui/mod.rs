mod actions;
pub mod app;
mod async_ops;
pub(crate) mod event;
pub(crate) mod types;
pub(crate) mod ui;

use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;

use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, SetTitle, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::mpsc;

use pike_core::manager::PackageManager;
use pike_core::package::{SourceType, StatusSummary};

use app::App;
use async_ops::AsyncResult;
use event::{Event, EventHandler};
use types::{HitState, ViewState};

pub async fn run(
    manager: &PackageManager,
    config_path: &Path,
    start_tab: Option<app::Tab>,
) -> anyhow::Result<()> {
    let cached = manager
        .get_cached_status()
        .unwrap_or_else(|_| StatusSummary::from_updates(vec![]));

    let active_sources = manager.active_source_types();

    let mut app = App::new(
        manager.config().clone(),
        cached.updates,
        active_sources.clone(),
    );
    if let Some(tab) = start_tab {
        app.tab = tab;
    }

    let events = EventHandler::new(Duration::from_millis(250));

    let mut terminal = setup_terminal()?;

    let result = run_loop(
        &mut terminal,
        &mut app,
        &events,
        manager,
        &active_sources,
        config_path,
    )
    .await;

    let _ = restore_terminal(&mut terminal);

    result
}

async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    events: &EventHandler,
    manager: &PackageManager,
    active_sources: &[SourceType],
    config_path: &Path,
) -> anyhow::Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<AsyncResult>();
    let mut hit = HitState::default();
    let mut view = ViewState::new(!app.updates.items.is_empty());

    app.refresh_daemon_status();
    async_ops::spawn_check_updates(app, &tx, active_sources);
    async_ops::spawn_list_installed(app, &tx, active_sources);

    while app.running {
        process_async_results(app, &mut rx, manager, &tx, active_sources, &mut view);

        app.ensure_settings_cache();
        hit.clear();
        terminal.draw(|frame| ui::render(frame, app, &mut view, &mut hit))?;

        let actions = match events.next()? {
            Event::Key(key) => app.handle_key(key, &mut view),
            Event::Mouse(mouse) => app.handle_mouse(mouse, &hit, &mut view),
            Event::Tick => {
                view.tick();
                continue;
            }
        };
        for action in actions {
            actions::handle_action(
                terminal,
                app,
                action,
                manager,
                &tx,
                active_sources,
                config_path,
            )
            .await?;
        }
    }
    Ok(())
}

fn process_async_results(
    app: &mut App,
    rx: &mut mpsc::UnboundedReceiver<AsyncResult>,
    manager: &PackageManager,
    tx: &mpsc::UnboundedSender<AsyncResult>,
    active_sources: &[SourceType],
    view: &mut ViewState,
) {
    while let Ok(result) = rx.try_recv() {
        match result {
            AsyncResult::SearchResults(results) => {
                app.set_search_results(results, view);
            }
            AsyncResult::Updates(updates) => {
                if let Err(e) = manager.cache_updates(&updates) {
                    tracing::warn!("failed to cache updates: {e}");
                }
                app.set_updates(updates, view);
                app.updates.loading = false;
            }
            AsyncResult::Installed(packages) => {
                app.set_installed(packages, view);
            }
            AsyncResult::Repos(repos) => {
                app.set_repos(repos, view);
            }
        }
    }

    if app.needs_installed_load() {
        async_ops::spawn_list_installed(app, tx, active_sources);
    }

    if app.needs_repos_load() {
        async_ops::spawn_list_repos(app, tx, active_sources);
    }
}

fn setup_terminal() -> anyhow::Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    crossterm::execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        SetTitle("pike")
    )?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;
    Ok(terminal)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> anyhow::Result<()> {
    disable_raw_mode()?;
    crossterm::execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        SetTitle("")
    )?;
    let _ = write!(terminal.backend_mut(), "\x1b]22;default\x07");
    terminal.show_cursor()?;
    Ok(())
}
