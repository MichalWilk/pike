use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use pike_core::package::StatusSummary;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonRequest {
    Status,
    Check {
        #[serde(default)]
        notify: bool,
        #[serde(default)]
        notify_always: bool,
    },
    Subscribe,
    ReloadConfig,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DaemonResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<StatusSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl DaemonResponse {
    pub fn ok() -> Self {
        Self {
            ok: true,
            status: None,
            error: None,
        }
    }

    pub fn success(status: StatusSummary) -> Self {
        Self {
            ok: true,
            status: Some(status),
            error: None,
        }
    }

    pub fn err(msg: String) -> Self {
        Self {
            ok: false,
            status: None,
            error: Some(msg),
        }
    }
}

pub fn socket_path() -> PathBuf {
    std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
        .join("pike.sock")
}

pub fn notify_daemon_recheck() {
    try_daemon_request(&DaemonRequest::Check {
        notify: false,
        notify_always: false,
    });
}

pub fn is_daemon_running() -> bool {
    UnixStream::connect(socket_path()).is_ok()
}

pub fn try_daemon_request(req: &DaemonRequest) -> Option<DaemonResponse> {
    let path = socket_path();
    let mut stream = UnixStream::connect(&path).ok()?;

    let timeout = match req {
        DaemonRequest::Check { .. } => Duration::from_secs(120),
        DaemonRequest::Status | DaemonRequest::Subscribe | DaemonRequest::ReloadConfig => {
            Duration::from_secs(5)
        }
    };
    if let Err(e) = stream.set_read_timeout(Some(timeout)) {
        tracing::warn!("daemon socket config failed: {e}");
        return None;
    }
    if let Err(e) = stream.set_write_timeout(Some(Duration::from_secs(5))) {
        tracing::warn!("daemon socket config failed: {e}");
        return None;
    }

    let mut payload = match serde_json::to_string(req) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("failed to serialize daemon request: {e}");
            return None;
        }
    };
    payload.push('\n');
    if let Err(e) = stream.write_all(payload.as_bytes()) {
        tracing::warn!("failed to write to daemon socket: {e}");
        return None;
    }

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    if let Err(e) = reader.read_line(&mut line) {
        tracing::warn!("failed to read daemon response: {e}");
        return None;
    }

    match serde_json::from_str(&line) {
        Ok(resp) => Some(resp),
        Err(e) => {
            tracing::warn!("failed to parse daemon response: {e}");
            None
        }
    }
}
