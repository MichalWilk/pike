rust_i18n::i18n!("locales", fallback = "en");

mod commands;
mod daemon;
mod i18n;
mod ipc;
mod tui;

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use pike_core::config::{Config, IconStyle};
use pike_core::db::Database;
use pike_core::manager::PackageManager;
use tracing_subscriber::prelude::*;

#[derive(Parser)]
#[command(name = "pike", about = "Unified package manager for Linux", version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    #[arg(help = "Output results as JSON", long, global = true)]
    json: bool,

    #[arg(help = "Enable debug logging", short, long, global = true)]
    verbose: bool,
}

#[derive(Subcommand)]
enum Commands {
    #[command(
        about = "Search packages across dnf and flatpak",
        visible_alias = "s",
        alias = "find"
    )]
    Search {
        #[arg(help = "Package name or keyword to search for")]
        query: String,
        #[arg(help = "Filter by source: dnf or flatpak", long, short = 'S')]
        source: Option<String>,
    },
    #[command(about = "Install a package", visible_alias = "i", alias = "add")]
    Install {
        #[arg(help = "Package name or flatpak app ID (e.g. org.mozilla.firefox)")]
        package: String,
        #[arg(help = "Force source: dnf or flatpak", long, short = 'S')]
        source: Option<String>,
    },
    #[command(about = "Remove a package", visible_alias = "rm", aliases = ["uninstall", "erase"])]
    Remove {
        #[arg(help = "Package name or flatpak app ID")]
        package: String,
        #[arg(help = "Force source: dnf or flatpak", long, short = 'S')]
        source: Option<String>,
        #[arg(
            help = "Also remove application data and configs (flatpak --delete-data)",
            long,
            short = 'p'
        )]
        purge: bool,
    },
    #[command(
        about = "Update one or all packages",
        visible_alias = "up",
        alias = "upgrade"
    )]
    Update {
        #[arg(help = "Package to update (updates all if omitted)")]
        package: Option<String>,
    },
    #[command(
        about = "Remove orphaned dependencies and unused runtimes",
        visible_alias = "ar",
        alias = "clean"
    )]
    Autoremove,
    #[command(about = "Check for available updates", visible_alias = "ck")]
    Check {
        #[arg(help = "Send desktop notification when updates are available", long)]
        notify: bool,
        #[arg(help = "Send desktop notification regardless of result", long)]
        notify_always: bool,
        #[arg(help = "Output single-line JSON for waybar after checking", long)]
        waybar: bool,
    },
    #[command(
        about = "List installed packages or available updates",
        visible_alias = "ls"
    )]
    List {
        #[arg(help = "Only show packages with available updates", long, short = 'u')]
        updates: bool,
    },
    #[command(
        about = "Show update summary (or JSON for waybar)",
        visible_alias = "st"
    )]
    Status {
        #[arg(help = "Output single-line JSON for waybar custom module", long)]
        waybar: bool,
        #[arg(help = "Send desktop notification when updates are available", long)]
        notify: bool,
        #[arg(help = "Send desktop notification regardless of result", long)]
        notify_always: bool,
    },
    #[command(about = "Manage repositories and remotes")]
    Repo {
        #[command(subcommand)]
        command: RepoCommands,
    },
    #[command(about = "Run background daemon for update checking and notifications")]
    Daemon,
    #[command(about = "Continuous waybar output (requires daemon)")]
    Waybar,
    #[command(about = "Launch interactive TUI", visible_alias = "ui")]
    Tui {
        #[arg(
            help = "Start on a specific tab (search, installed, updates, repos, settings)",
            long,
            short
        )]
        tab: Option<String>,
    },
}

#[derive(Subcommand)]
pub(crate) enum RepoCommands {
    #[command(about = "List all repositories and remotes", visible_alias = "ls")]
    List {
        #[arg(help = "Filter by source: dnf or flatpak", long, short = 'S')]
        source: Option<String>,
    },
    #[command(about = "Enable a repository or remote")]
    Enable {
        #[arg(help = "Repository ID or remote name")]
        repo_id: String,
        #[arg(help = "Force source: dnf or flatpak", long, short = 'S')]
        source: Option<String>,
    },
    #[command(about = "Disable a repository or remote")]
    Disable {
        #[arg(help = "Repository ID or remote name")]
        repo_id: String,
        #[arg(help = "Force source: dnf or flatpak", long, short = 'S')]
        source: Option<String>,
    },
    #[command(
        about = "Add a repository (.repo URL, COPR, base URL, RPM for dnf; remote for flatpak)"
    )]
    Add {
        #[arg(help = "Remote name (flatpak) or display name")]
        name: String,
        #[arg(help = "URL or owner/project (COPR)")]
        url: String,
        #[arg(help = "Source: dnf or flatpak", long, short = 'S')]
        source: String,
        #[arg(
            help = "Method: repofile, copr, baseurl, rpm, remote (default: repofile for dnf, remote for flatpak)",
            long,
            short
        )]
        method: Option<String>,
        #[arg(help = "Repository ID (dnf repofile/baseurl)", long)]
        repo_id: Option<String>,
    },
    #[command(about = "Remove a repository or remote", visible_alias = "rm")]
    Remove {
        #[arg(help = "Repository ID or remote name")]
        repo_id: String,
        #[arg(help = "Force source: dnf or flatpak", long, short = 'S')]
        source: Option<String>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let config = Config::load()?;
    pike_core::source::set_privilege_method(config.general.privilege_escalation);
    let locale = if config.display.language == "auto" {
        let lang = sys_locale::get_locale().unwrap_or_else(|| String::from("en"));
        lang.split(['-', '_']).next().unwrap_or("en").to_string()
    } else {
        config.display.language.clone()
    };
    rust_i18n::set_locale(&locale);
    let is_tui = matches!(cli.command, Commands::Tui { .. });
    setup_tracing(cli.verbose, &config, is_tui)?;
    let icon_style = IconStyle::detect();
    let db = Database::new(&Database::default_path()?)?;
    db.migrate()?;
    let manager = PackageManager::new(config, db).await?;

    match cli.command {
        Commands::Search { query, source } => {
            commands::search(&manager, &query, source.as_deref(), cli.json).await?;
        }
        Commands::Install { package, source } => {
            commands::install(&manager, &package, source.as_deref()).await?;
        }
        Commands::Remove {
            package,
            source,
            purge,
        } => {
            commands::remove(&manager, &package, source.as_deref(), purge).await?;
        }
        Commands::Update { package } => {
            commands::update(&manager, package.as_deref()).await?;
        }
        Commands::Autoremove => {
            commands::autoremove(&manager).await?;
        }
        Commands::Check {
            notify,
            notify_always,
            waybar,
        } => {
            commands::check(
                &manager,
                cli.json,
                notify || notify_always,
                notify_always,
                waybar,
                icon_style,
            )
            .await?;
        }
        Commands::List { updates } => {
            commands::list(&manager, updates, cli.json).await?;
        }
        Commands::Status {
            waybar,
            notify,
            notify_always,
        } => {
            commands::status(
                &manager,
                waybar,
                notify || notify_always,
                notify_always,
                cli.json,
                icon_style,
            )?;
        }
        Commands::Repo { command } => {
            commands::repo(&manager, command, cli.json).await?;
        }
        Commands::Daemon => {
            daemon::run(manager).await?;
        }
        Commands::Waybar => {
            commands::waybar_continuous(icon_style)?;
        }
        Commands::Tui { tab } => {
            let start_tab = tab.as_deref().map(parse_tab).transpose()?;
            tui::run(&manager, &Config::path()?, start_tab).await?;
        }
    }

    Ok(())
}

fn setup_tracing(verbose: bool, config: &Config, is_tui: bool) -> anyhow::Result<()> {
    let filter = if verbose { "pike=debug" } else { "pike=warn" };

    let stderr_layer = if is_tui {
        None
    } else {
        Some(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .with_filter(tracing_subscriber::EnvFilter::new(filter)),
        )
    };

    let file_layer = if config.logging.file || is_tui {
        let log_dir = dirs::state_dir()
            .unwrap_or_else(|| dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("/tmp")))
            .join("pike");
        std::fs::create_dir_all(&log_dir)?;
        let log_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_dir.join("pike.log"))?;

        let file_filter = if is_tui && verbose {
            "pike=debug"
        } else if is_tui {
            "pike=warn"
        } else {
            "pike=info"
        };

        Some(
            tracing_subscriber::fmt::layer()
                .with_writer(std::sync::Mutex::new(log_file))
                .with_ansi(false)
                .with_filter(tracing_subscriber::EnvFilter::new(file_filter)),
        )
    } else {
        None
    };

    tracing_subscriber::registry()
        .with(stderr_layer)
        .with(file_layer)
        .init();
    Ok(())
}

fn parse_tab(s: &str) -> anyhow::Result<tui::app::Tab> {
    match s {
        "search" | "s" | "1" => Ok(tui::app::Tab::Search),
        "installed" | "i" | "2" => Ok(tui::app::Tab::Installed),
        "updates" | "u" | "3" => Ok(tui::app::Tab::Updates),
        "repos" | "r" | "4" => Ok(tui::app::Tab::Repos),
        "settings" | "9" => Ok(tui::app::Tab::Settings),
        "about" | "0" => Ok(tui::app::Tab::About),
        other => anyhow::bail!(
            "unknown tab '{}', expected: search, installed, updates, repos, settings, about",
            other
        ),
    }
}
