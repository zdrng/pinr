#![forbid(unsafe_code)]

mod cli;
mod config;
mod discover;
mod domain;
#[cfg(test)]
mod functional_tests;
mod infra;
#[cfg(test)]
mod mock;
mod service;
mod tui;

use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};

use crate::infra::fs::RealFs;
use crate::infra::http::ReqwestHttp;
use crate::service::PinService;

/// Pin GitHub Actions, GitLab CI includes and container images to immutable hashes.
///
/// Without a command, opens the interactive TUI.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Directory to scan.
    #[arg(short = 'C', long, default_value = ".", global = true)]
    dir: PathBuf,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// List unpinned references and exit non-zero if there are any. Offline.
    Check,
    /// Pin every unpinned reference.
    Pin {
        /// Only show what would change.
        #[arg(long)]
        dry_run: bool,
    },
}

#[tokio::main]
async fn main() -> Result<ExitCode> {
    let cli = Cli::parse();
    if cli.command.is_none() && !std::io::stdout().is_terminal() {
        Cli::command().print_help()?;
        return Ok(ExitCode::SUCCESS);
    }
    let config = config::load(&cli.dir)?;
    let files = discover::discover(&cli.dir);
    let service = PinService::new(RealFs, ReqwestHttp::new()?, config);
    let occurrences = service.scan(&files);

    match cli.command {
        Some(Command::Check) => Ok(cli::check(&occurrences)),
        Some(Command::Pin { dry_run }) => cli::pin(&service, &occurrences, dry_run).await,
        None => {
            tui::run(Arc::new(service), occurrences).await?;
            Ok(ExitCode::SUCCESS)
        }
    }
}
