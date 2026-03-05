use pike_core::manager::PackageManager;
use pike_core::package::StatusSummary;
use rust_i18n::t;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::net::unix::OwnedWriteHalf;

use crate::ipc::{DaemonRequest, DaemonResponse, socket_path};

struct DaemonState {
    manager: PackageManager,
    cached: Option<StatusSummary>,
    subscribers: Vec<OwnedWriteHalf>,
    interval_secs: u64,
    notify: bool,
    interval_changed: bool,
}

pub async fn run(manager: PackageManager) -> anyhow::Result<()> {
    let sock = socket_path();
    if sock.exists() {
        std::fs::remove_file(&sock)?;
    }

    let listener = UnixListener::bind(&sock)?;
    tracing::info!("daemon listening on {}", sock.display());
    eprintln!("  {}", t!("daemon.listening", path = sock.display()));

    let interval_secs = manager.config().daemon.interval.max(10);
    let notify = manager.config().daemon.notify;

    let mut state = DaemonState {
        manager,
        cached: None,
        subscribers: Vec::new(),
        interval_secs,
        notify,
        interval_changed: false,
    };

    let mut interval = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
    interval.tick().await;

    loop {
        tokio::select! {
            _ = interval.tick() => {
                tracing::debug!("periodic check triggered");
                if let Err(e) = run_check(&mut state, true, false).await {
                    tracing::warn!("periodic check failed: {e}");
                }
            }
            result = listener.accept() => {
                match result {
                    Ok((stream, _)) => {
                        if let Err(e) = handle_client(stream, &mut state).await {
                            tracing::warn!("client error: {e}");
                        }
                    }
                    Err(e) => {
                        tracing::warn!("accept error: {e}");
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("shutting down");
                break;
            }
        }

        if state.interval_changed {
            interval = tokio::time::interval(std::time::Duration::from_secs(state.interval_secs));
            interval.tick().await;
            state.interval_changed = false;
            tracing::info!("check interval changed to {}s", state.interval_secs);
        }
    }

    let _ = std::fs::remove_file(&sock);
    Ok(())
}

async fn run_check(
    state: &mut DaemonState,
    notify: bool,
    notify_always: bool,
) -> anyhow::Result<StatusSummary> {
    let updates = state.manager.check_updates().await?;
    let status = StatusSummary::from_fresh_check(updates);

    let should_notify = if notify_always {
        true
    } else if notify && status.total > 0 {
        state
            .cached
            .as_ref()
            .is_none_or(|p| p.total != status.total)
    } else {
        false
    };

    if should_notify && state.notify {
        send_notification(&status);
    }

    state.cached = Some(status.clone());
    push_to_subscribers(&mut state.subscribers, &status).await;
    Ok(status)
}

async fn push_to_subscribers(subscribers: &mut Vec<OwnedWriteHalf>, status: &StatusSummary) {
    let response = DaemonResponse::success(status.clone());
    let mut payload = match serde_json::to_string(&response) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("failed to serialize subscriber payload: {e}");
            return;
        }
    };
    payload.push('\n');
    let bytes = payload.as_bytes();

    let mut alive = Vec::new();
    for mut sub in subscribers.drain(..) {
        if sub.write_all(bytes).await.is_ok() {
            alive.push(sub);
        } else {
            tracing::debug!("subscriber disconnected");
        }
    }
    *subscribers = alive;
}

fn get_current_status(state: &DaemonState) -> DaemonResponse {
    match &state.cached {
        Some(s) => DaemonResponse::success(s.clone()),
        None => match state.manager.get_cached_status() {
            Ok(s) => DaemonResponse::success(s),
            Err(e) => DaemonResponse::err(e.to_string()),
        },
    }
}

fn reload_config(state: &mut DaemonState) -> DaemonResponse {
    match pike_core::config::Config::load() {
        Ok(config) => {
            let new_interval = config.daemon.interval.max(10);
            let new_notify = config.daemon.notify;
            if new_interval != state.interval_secs {
                state.interval_secs = new_interval;
                state.interval_changed = true;
            }
            state.notify = new_notify;
            tracing::info!(
                "config reloaded: interval={}s, notify={}",
                state.interval_secs,
                state.notify
            );
            DaemonResponse::ok()
        }
        Err(e) => DaemonResponse::err(e.to_string()),
    }
}

async fn handle_client(
    stream: tokio::net::UnixStream,
    state: &mut DaemonState,
) -> anyhow::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    reader.read_line(&mut line).await?;

    let req: DaemonRequest = serde_json::from_str(line.trim())?;
    tracing::debug!("request: {req:?}");

    match req {
        DaemonRequest::Subscribe => {
            let response = get_current_status(state);
            let mut payload = serde_json::to_string(&response)?;
            payload.push('\n');
            writer.write_all(payload.as_bytes()).await?;
            state.subscribers.push(writer);
            tracing::debug!("subscriber added (total: {})", state.subscribers.len());
        }
        _ => {
            let response = match req {
                DaemonRequest::Status => get_current_status(state),
                DaemonRequest::Check {
                    notify,
                    notify_always,
                } => match run_check(state, notify, notify_always).await {
                    Ok(s) => DaemonResponse::success(s),
                    Err(e) => DaemonResponse::err(e.to_string()),
                },
                DaemonRequest::ReloadConfig => reload_config(state),
                DaemonRequest::Subscribe => DaemonResponse::err("unexpected request".into()),
            };

            let mut payload = serde_json::to_string(&response)?;
            payload.push('\n');
            writer.write_all(payload.as_bytes()).await?;
        }
    }

    Ok(())
}

fn send_notification(status: &StatusSummary) {
    if status.total == 0 {
        crate::commands::send_notification(status);
        return;
    }

    let title = t!("cli.notify-title", count = status.total).to_string();
    let body = crate::commands::format_update_counts(status);

    std::thread::spawn(move || {
        let mut cmd = std::process::Command::new("notify-send");
        cmd.args(["-a", "Pike", "-u", "low"]);
        let output = cmd
            .arg(format!("--action=update={}", t!("notify.update")))
            .arg(format!("--action=dismiss={}", t!("notify.dismiss")))
            .arg(&title)
            .arg(&body)
            .output();
        match output {
            Ok(out) => {
                let action = String::from_utf8_lossy(&out.stdout);
                if action.trim() == "update" {
                    let terminal = std::env::var("TERMINAL").unwrap_or_else(|_| "foot".into());
                    if let Err(e) = std::process::Command::new(&terminal)
                        .args(["-e", "pike", "tui", "--tab", "updates"])
                        .spawn()
                    {
                        tracing::warn!("failed to launch terminal '{terminal}': {e}");
                    }
                }
            }
            Err(e) => {
                tracing::warn!("failed to send notification: {e}");
            }
        }
    });
}
