//! Non-interactive frontends: `pinr check` and `pinr pin`.

use std::process::ExitCode;

use anyhow::Result;

use crate::domain::model::{Kind, Occurrence};
use crate::infra::fs::FileSystem;
use crate::infra::http::Http;
use crate::service::{PinService, unpinned};

/// `path:line`, without a leading `./`.
pub fn location(occ: &Occurrence) -> String {
    let path = occ.path.strip_prefix("./").unwrap_or(&occ.path);
    format!("{}:{}", path.display(), occ.line)
}

fn print_warnings(occurrences: &[Occurrence]) -> usize {
    let mut count = 0;
    for occ in occurrences {
        if let Kind::Warning(reason) = &occ.kind {
            eprintln!("warning: {}  {reason}", location(occ));
            count += 1;
        }
    }
    count
}

/// Fails if anything is unpinned. Offline.
pub fn check(occurrences: &[Occurrence]) -> ExitCode {
    let warnings = print_warnings(occurrences);
    let mut count = 0;
    for occ in unpinned(occurrences) {
        println!("unpinned: {}  {}", location(occ), occ.reference().unwrap());
        count += 1;
    }
    println!("{count} unpinned, {warnings} warnings");
    if count > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Resolve and pin everything. Fails if any reference couldn't be resolved.
pub async fn pin<F: FileSystem, H: Http>(
    service: &PinService<F, H>,
    occurrences: &[Occurrence],
    dry_run: bool,
) -> Result<ExitCode> {
    print_warnings(occurrences);
    let resolutions = service.resolve_all(occurrences).await;

    let mut failed = 0;
    let targets: Vec<&Occurrence> = unpinned(occurrences).collect();
    for occ in &targets {
        let reference = occ.reference().unwrap();
        match &resolutions[reference] {
            Ok(res) => println!("{}  {reference}  ->  {res}", location(occ)),
            Err(e) => {
                eprintln!("error: {}  {reference}: {e}", location(occ));
                failed += 1;
            }
        }
    }

    let edits = service.plan(&targets, &resolutions)?;
    if dry_run {
        println!(
            "dry run: {} references would be pinned",
            targets.len() - failed
        );
    } else {
        let files = service.apply(&edits)?;
        println!(
            "pinned {} references in {files} files",
            targets.len() - failed
        );
    }
    Ok(if failed > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}
