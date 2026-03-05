pub mod dnf;
pub mod flatpak;

use async_trait::async_trait;

use crate::error::PikeError;
use crate::package::{Package, PackageUpdate, RepoMethod, Repository, SourceType};

pub type Result<T> = std::result::Result<T, PikeError>;

#[async_trait]
pub trait PackageSource: Send + Sync {
    fn name(&self) -> &str;
    fn source_type(&self) -> SourceType;
    async fn search(&self, query: &str) -> Result<Vec<Package>>;
    async fn install(&self, package: &str) -> Result<()>;
    async fn remove(&self, package: &str, purge: bool) -> Result<()>;

    async fn autoremove(&self) -> Result<()>;
    async fn check_updates(&self) -> Result<Vec<PackageUpdate>>;
    async fn update(&self, package: &str) -> Result<()>;
    async fn update_all(&self) -> Result<()>;
    async fn list_installed(&self) -> Result<Vec<Package>>;

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
}

pub fn create_sources(active: &[SourceType]) -> Vec<Box<dyn PackageSource>> {
    active
        .iter()
        .map(|st| -> Box<dyn PackageSource> {
            match st {
                SourceType::Dnf => Box::new(dnf::DnfSource),
                SourceType::Flatpak => Box::new(flatpak::FlatpakSource),
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
    tracing::debug!("exec: {}", format_cmd(cmd, args));
    let output = tokio::process::Command::new(cmd)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?
        .wait_with_output()
        .await?;
    let code = output.status.code().unwrap_or(-1);
    if !output.status.success() && !allowed_codes.contains(&code) {
        return Err(PikeError::CommandFailed {
            cmd: format_cmd(cmd, args),
            exit_code: code,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
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
