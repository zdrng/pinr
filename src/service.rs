//! Orchestrates scan → resolve → plan → apply. CLI and TUI only talk to this.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

use anyhow::{Result, anyhow};
use futures::{Stream, StreamExt};

use crate::config::Config;
use crate::domain::model::{Ecosystem, Edit, Kind, Occurrence, Reference};
use crate::domain::{edit, extract};
use crate::infra::fs::FileSystem;
use crate::infra::http::Http;
use crate::provider::git::Git;
use crate::provider::oci::Oci;
use crate::provider::{self, Resolution};

/// How many references are resolved concurrently.
const CONCURRENCY: usize = 8;

pub type Resolutions = HashMap<Reference, Result<Resolution, String>>;

pub struct PinService<F, H> {
    fs: F,
    http: H,
    config: Config,
}

impl<F: FileSystem, H: Http> PinService<F, H> {
    pub fn new(fs: F, http: H, config: Config) -> Self {
        PinService { fs, http, config }
    }

    #[cfg(test)]
    pub fn fs(&self) -> &F {
        &self.fs
    }

    #[cfg(test)]
    pub fn http(&self) -> &H {
        &self.http
    }

    /// Find all references and warnings in the given files. Local and fast.
    pub fn scan(&self, files: &[(PathBuf, Ecosystem)]) -> Vec<Occurrence> {
        let mut occurrences = Vec::new();
        for (path, ecosystem) in files {
            let src = match self.fs.read(path) {
                Ok(src) => src,
                Err(e) => {
                    occurrences.push(Occurrence {
                        path: path.clone(),
                        ecosystem: *ecosystem,
                        line: 0,
                        span: 0..0,
                        raw: String::new(),
                        kind: Kind::Warning(format!("{e:#}")),
                    });
                    continue;
                }
            };
            let extractor = extract::for_ecosystem(*ecosystem, &self.config);
            for found in extractor.extract(&src) {
                occurrences.push(Occurrence {
                    path: path.clone(),
                    ecosystem: *ecosystem,
                    line: src[..found.span.start].matches('\n').count() + 1,
                    raw: src[found.span.clone()].to_string(),
                    span: found.span,
                    kind: found.kind,
                });
            }
        }
        occurrences.retain(|o| !self.is_ignored(o));
        occurrences
    }

    fn is_ignored(&self, occ: &Occurrence) -> bool {
        let path = occ.path.to_string_lossy();
        let reference = occ.reference().map(|r| r.to_string()).unwrap_or_default();
        self.config
            .ignore
            .iter()
            .any(|pattern| path.contains(pattern.as_str()) || reference.contains(pattern.as_str()))
    }

    /// The one place that maps a kind of reference to its provider.
    pub async fn resolve(&self, reference: &Reference) -> Result<Resolution> {
        match reference {
            Reference::Git(r) => provider::resolve(&Git, &self.http, r).await,
            Reference::Image(r) => provider::resolve(&Oci, &self.http, r).await,
        }
    }

    /// Resolve every unpinned reference once, no matter how often it occurs,
    /// yielding results as they arrive.
    pub fn resolve_each<'s>(
        &'s self,
        occurrences: &[Occurrence],
    ) -> impl Stream<Item = (Reference, Result<Resolution, String>)> + use<'s, F, H> {
        let unique: HashSet<Reference> = unpinned(occurrences)
            .filter_map(|o| o.reference())
            .cloned()
            .collect();
        futures::stream::iter(unique)
            .map(move |r| async move {
                let result = self.resolve(&r).await.map_err(|e| format!("{e:#}"));
                (r, result)
            })
            .buffer_unordered(CONCURRENCY)
    }

    pub async fn resolve_all(&self, occurrences: &[Occurrence]) -> Resolutions {
        self.resolve_each(occurrences).collect().await
    }

    /// Edits for every occurrence that has a successful resolution.
    pub fn plan(
        &self,
        occurrences: &[&Occurrence],
        resolutions: &Resolutions,
    ) -> Result<Vec<Edit>> {
        let mut sources = HashMap::new();
        let mut edits = Vec::new();
        for occ in occurrences {
            let Some(Ok(res)) = occ.reference().and_then(|r| resolutions.get(r)) else {
                continue;
            };
            if !sources.contains_key(&occ.path) {
                sources.insert(occ.path.clone(), self.fs.read(&occ.path)?);
            }
            edits.extend(res.edits(occ, &sources[&occ.path]));
        }
        Ok(edits)
    }

    /// Write edits to disk. Returns the number of changed files. Every file is
    /// checked before any is written, so a stale file leaves all of them untouched.
    pub fn apply(&self, edits: &[Edit]) -> Result<usize> {
        let mut by_file: BTreeMap<&PathBuf, Vec<&Edit>> = BTreeMap::new();
        for edit in edits {
            by_file.entry(&edit.path).or_default().push(edit);
        }
        let mut updated = Vec::new();
        for (path, edits) in by_file {
            let src = self.fs.read(path)?;
            updated.push((path, edit::apply(&src, &edits).map_err(|e| anyhow!(e))?));
        }
        for (path, new) in &updated {
            self.fs.write(path, new)?;
        }
        Ok(updated.len())
    }
}

/// Occurrences that reference something not yet pinned.
pub fn unpinned(occurrences: &[Occurrence]) -> impl Iterator<Item = &Occurrence> {
    occurrences
        .iter()
        .filter(|o| o.reference().is_some_and(|r| !r.is_pinned()))
}
