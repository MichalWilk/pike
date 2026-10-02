pub mod apt;
pub mod dnf;
pub mod flatpak;

use std::io::IsTerminal;
use std::sync::OnceLock;

use async_trait::async_trait;

use crate::config::PrivilegeEscalation;
use crate::error::PikeError;
use crate::package::{
    CleanupItem, CleanupKind, Package, PackageUpdate, RepoMethod, Repository, SourceType,
};

static PRIVILEGE_METHOD: OnceLock<PrivilegeEscalation> = OnceLock::new();

pub fn set_privilege_method(method: PrivilegeEscalation) {
    let _ = PRIVILEGE_METHOD.set(method);
}

pub type Result<T> = std::result::Result<T, PikeError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingGpgKey {
    pub key_id: String,
    pub user_id: String,
    pub fingerprint: String,
}

#[async_trait]
pub trait PackageSource: Send + Sync {
    fn name(&self) -> &str;
    fn source_type(&self) -> SourceType;
    async fn search(&self, query: &str) -> Result<Vec<Package>>;
    async fn install(&self, package: &str) -> Result<()>;
    async fn remove(&self, package: &str, purge: bool) -> Result<()>;

    async fn check_updates(&self) -> Result<Vec<PackageUpdate>>;
    async fn update(&self, package: &str) -> Result<()>;
    async fn update_all(&self) -> Result<()>;
    async fn list_installed(&self) -> Result<Vec<Package>>;

    async fn install_many(&self, packages: &[String]) -> Result<()> {
        for pkg in packages {
            self.install(pkg).await?;
        }
        Ok(())
    }

    async fn remove_many(&self, packages: &[String], purge: bool) -> Result<()> {
        for pkg in packages {
            self.remove(pkg, purge).await?;
        }
        Ok(())
    }

    async fn update_many(&self, packages: &[String]) -> Result<()> {
        for pkg in packages {
            self.update(pkg).await?;
        }
        Ok(())
    }

    async fn list_repos(&self) -> Result<Vec<Repository>> {
        Err(PikeError::Other(format!(
            "{} does not support listing repos",
            self.name()
        )))
    }

    async fn set_repo_enabled(&self, _id: &str, _enabled: bool) -> Result<()> {
        Err(PikeError::Other(format!(
            "{} does not support toggling repos",
            self.name()
        )))
    }

    async fn add_repo(
        &self,
        _method: RepoMethod,
        _repo_id: &str,
        _name: &str,
        _url: &str,
        _gpgcheck: bool,
    ) -> Result<()> {
        Err(PikeError::Other(format!(
            "{} does not support adding repos",
            self.name()
        )))
    }

    async fn remove_repo(&self, _id: &str) -> Result<()> {
        Err(PikeError::Other(format!(
            "{} does not support removing repos",
            self.name()
        )))
    }

    async fn refresh_preflight(&self) -> Result<Vec<PendingGpgKey>> {
        Ok(Vec::new())
    }

    async fn import_keys(&self) -> Result<()> {
        Ok(())
    }

    /// Only the requested `kinds` are queried.
    async fn list_cleanup(
        &self,
        _kinds: &[CleanupKind],
        _keep_kernels: usize,
    ) -> Result<Vec<CleanupItem>> {
        Ok(Vec::new())
    }

    async fn clean(&self, _items: &[CleanupItem]) -> Result<()> {
        Err(PikeError::Other(format!(
            "{} does not support cleanup",
            self.name()
        )))
    }

    /// Packages `clean(items)` would remove beyond `items` themselves.
    async fn preview_clean(&self, _items: &[CleanupItem]) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
}

/// Sources whose preflight fails are logged and skipped.
pub async fn pending_gpg_keys(
    sources: &[&dyn PackageSource],
) -> Vec<(SourceType, Vec<PendingGpgKey>)> {
    let futures: Vec<_> = sources.iter().map(|s| s.refresh_preflight()).collect();
    let results = futures::future::join_all(futures).await;
    let mut pending = Vec::new();
    for (source, result) in sources.iter().zip(results) {
        match result {
            Ok(keys) if !keys.is_empty() => pending.push((source.source_type(), keys)),
            Ok(_) => {}
            Err(e) => tracing::warn!("refresh_preflight {} failed: {}", source.name(), e),
        }
    }
    pending
}

pub fn create_sources(active: &[SourceType]) -> Vec<Box<dyn PackageSource>> {
    active
        .iter()
        .map(|st| -> Box<dyn PackageSource> {
            match st {
                SourceType::Dnf => Box::new(dnf::DnfSource),
                SourceType::Flatpak => Box::new(flatpak::FlatpakSource),
                SourceType::Apt => Box::new(apt::AptSource),
            }
        })
        .collect()
}

pub(crate) fn parse_installed_versions(
    output: &str,
    delimiter: char,
) -> std::collections::HashMap<&str, String> {
    output
        .lines()
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            let (key, version) = line.split_once(delimiter)?;
            if key.is_empty() {
                return None;
            }
            Some((key, version.to_string()))
        })
        .collect()
}

fn format_cmd(cmd: &str, args: &[&str]) -> String {
    format!("{} {}", cmd, args.join(" "))
}

pub async fn run_captured(cmd: &str, args: &[&str]) -> Result<String> {
    run_captured_allow_exit(cmd, args, &[]).await
}

pub async fn run_captured_allow_exit(
    cmd: &str,
    args: &[&str],
    allowed_codes: &[i32],
) -> Result<String> {
    Ok(run_captured_impl(cmd, args, None, false, allowed_codes)
        .await?
        .stdout)
}

pub(crate) async fn run_captured_c(
    cmd: &str,
    args: &[&str],
    allowed_codes: &[i32],
) -> Result<String> {
    Ok(run_captured_c_full(cmd, args, allowed_codes).await?.stdout)
}

pub(crate) struct CapturedOutput {
    pub stdout: String,
    pub stderr: String,
}

pub(crate) async fn run_captured_c_full(
    cmd: &str,
    args: &[&str],
    allowed_codes: &[i32],
) -> Result<CapturedOutput> {
    run_captured_impl(cmd, args, None, true, allowed_codes).await
}

async fn run_captured_impl(
    cmd: &str,
    args: &[&str],
    input: Option<&str>,
    c_locale: bool,
    allowed_codes: &[i32],
) -> Result<CapturedOutput> {
    use tokio::io::AsyncWriteExt;
    tracing::debug!("exec: {}", format_cmd(cmd, args));
    let mut command = tokio::process::Command::new(cmd);
    command
        .args(args)
        .stdin(if input.is_some() {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        })
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if c_locale {
        command.env("LC_ALL", "C");
    }
    let mut child = command.spawn()?;
    if let Some(input) = input
        && let Some(mut stdin) = child.stdin.take()
        && let Err(e) = stdin.write_all(input.as_bytes()).await
        && e.kind() != std::io::ErrorKind::BrokenPipe
    {
        return Err(e.into());
    }
    let output = child.wait_with_output().await?;
    let code = output.status.code().unwrap_or(-1);
    if !output.status.success() && !allowed_codes.contains(&code) {
        return Err(PikeError::CommandFailed {
            cmd: format_cmd(cmd, args),
            exit_code: code,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(CapturedOutput {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

pub async fn run_captured_stderr(cmd: &str, args: &[&str]) -> Result<String> {
    tracing::debug!("exec: {}", format_cmd(cmd, args));
    let output = tokio::process::Command::new(cmd)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?
        .wait_with_output()
        .await?;
    Ok(String::from_utf8_lossy(&output.stderr).into_owned())
}

/// `input` is written in full before output is read, so it must fit the pipe buffer.
pub(crate) async fn run_captured_with_input(
    cmd: &str,
    args: &[&str],
    input: &str,
    allowed_codes: &[i32],
) -> Result<CapturedOutput> {
    run_captured_impl(cmd, args, Some(input), true, allowed_codes).await
}

pub async fn run_interactive(cmd: &str, args: &[&str]) -> Result<()> {
    tracing::debug!("exec (interactive): {}", format_cmd(cmd, args));
    let status = tokio::process::Command::new(cmd)
        .args(args)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .await?;
    if !status.success() {
        return Err(PikeError::CommandFailed {
            cmd: format_cmd(cmd, args),
            exit_code: status.code().unwrap_or(-1),
            stderr: String::new(),
        });
    }
    Ok(())
}

pub async fn run_privileged(args: &[&str]) -> Result<()> {
    let cmd = resolve_privilege_cmd()?;
    run_interactive(cmd, args).await
}

fn resolve_privilege_cmd() -> Result<&'static str> {
    let method = PRIVILEGE_METHOD.get().copied().unwrap_or_default();
    match method {
        PrivilegeEscalation::Auto => {
            if std::io::stdin().is_terminal() {
                Ok("sudo")
            } else if binary_exists_sync("pkexec") {
                Ok("pkexec")
            } else {
                Err(PikeError::Other(
                    "no TTY available and pkexec not found; set privilege_escalation in config or run from a terminal".into(),
                ))
            }
        }
        PrivilegeEscalation::Sudo => {
            if std::io::stdin().is_terminal() {
                Ok("sudo")
            } else {
                Err(PikeError::Other(
                    "sudo requires a terminal; run from a terminal or set privilege_escalation = \"pkexec\" in config".into(),
                ))
            }
        }
        PrivilegeEscalation::Pkexec => Ok("pkexec"),
        PrivilegeEscalation::Doas => {
            if std::io::stdin().is_terminal() {
                Ok("doas")
            } else {
                Err(PikeError::Other(
                    "doas requires a terminal; run from a terminal or set privilege_escalation = \"pkexec\" in config".into(),
                ))
            }
        }
    }
}

fn binary_exists_sync(name: &str) -> bool {
    std::process::Command::new("which")
        .arg(name)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_run_captured_with_input() {
        let output = run_captured_with_input("cat", &[], "n\n", &[])
            .await
            .unwrap();
        assert_eq!(output.stdout, "n\n");
    }

    #[tokio::test]
    async fn test_run_captured_allowed_exit_code() {
        let args = ["-c", "echo out; echo err >&2; exit 1"];
        let output = run_captured_c_full("sh", &args, &[1]).await.unwrap();
        assert_eq!(output.stdout, "out\n");
        assert_eq!(output.stderr, "err\n");
        assert!(matches!(
            run_captured_c_full("sh", &args, &[]).await,
            Err(PikeError::CommandFailed { exit_code: 1, .. })
        ));
    }

    struct KeyStub {
        st: SourceType,
        keys: Option<Vec<PendingGpgKey>>,
    }

    #[async_trait]
    impl PackageSource for KeyStub {
        fn name(&self) -> &str {
            "stub"
        }
        fn source_type(&self) -> SourceType {
            self.st
        }
        async fn search(&self, _query: &str) -> Result<Vec<Package>> {
            Ok(Vec::new())
        }
        async fn install(&self, _package: &str) -> Result<()> {
            Ok(())
        }
        async fn remove(&self, _package: &str, _purge: bool) -> Result<()> {
            Ok(())
        }
        async fn check_updates(&self) -> Result<Vec<PackageUpdate>> {
            Ok(Vec::new())
        }
        async fn update(&self, _package: &str) -> Result<()> {
            Ok(())
        }
        async fn update_all(&self) -> Result<()> {
            Ok(())
        }
        async fn list_installed(&self) -> Result<Vec<Package>> {
            Ok(Vec::new())
        }
        async fn refresh_preflight(&self) -> Result<Vec<PendingGpgKey>> {
            self.keys
                .clone()
                .ok_or_else(|| PikeError::Other("stub failure".into()))
        }
    }

    #[tokio::test]
    async fn test_pending_gpg_keys_skips_empty_and_failing() {
        let key = PendingGpgKey {
            key_id: "0xDE226D6F".into(),
            user_id: "Terra 44 <security@fyralabs.com>".into(),
            fingerprint: "AE09157A4DE88B497EA1D5D300CDAB43DE226D6F".into(),
        };
        let with_keys = KeyStub {
            st: SourceType::Dnf,
            keys: Some(vec![key.clone(), key.clone()]),
        };
        let empty = KeyStub {
            st: SourceType::Flatpak,
            keys: Some(Vec::new()),
        };
        let failing = KeyStub {
            st: SourceType::Apt,
            keys: None,
        };
        let sources: [&dyn PackageSource; 3] = [&failing, &with_keys, &empty];
        let pending = pending_gpg_keys(&sources).await;
        assert_eq!(pending, vec![(SourceType::Dnf, vec![key.clone(), key])]);
    }
}
