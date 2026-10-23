//! Providers turn mutable references into immutable ones. There is one per
//! protocol, not per file format: GitHub Actions and GitLab includes both use git.

pub mod git;
pub mod oci;

use std::fmt;
use std::future::Future;
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;

use crate::domain::model::{Edit, Occurrence, VersionComment};
use crate::infra::http::Http;

/// Byte ranges of a file and the text that replaces them.
pub type Replacements = Vec<(Range<usize>, String)>;

pub trait Provider: Send + Sync + 'static {
    /// What extractors find, e.g. `actions/checkout@v4`.
    type Ref: Clone + Send + Sync + 'static;
    /// The immutable identifier it resolves to, e.g. a commit SHA.
    type Pin: fmt::Display + Send + Sync + 'static;

    fn resolve<H: Http>(
        &self,
        http: &H,
        reference: &Self::Ref,
    ) -> impl Future<Output = Result<Self::Pin>> + Send;

    /// Byte ranges of `src` to replace so that the reference found at `span`
    /// is pinned to `pin`. `comment` is how the file format writes versions
    /// next to a pin, for providers that add one.
    fn replacements(
        reference: &Self::Ref,
        pin: &Self::Pin,
        src: &str,
        span: Range<usize>,
        comment: VersionComment,
    ) -> Replacements;
}

/// Resolve `reference` with `provider`.
pub async fn resolve<P: Provider, H: Http>(
    provider: &P,
    http: &H,
    reference: &P::Ref,
) -> Result<Resolution> {
    let pin = provider.resolve(http, reference).await?;
    Ok(Resolution::new::<P>(reference.clone(), pin))
}

/// What a reference resolved to. The pin is always written using the reference
/// it was resolved from, so it can't be mixed up with another kind of reference.
#[derive(Clone)]
pub struct Resolution {
    pin: Arc<str>,
    replacements: Arc<ReplaceFn>,
}

/// `P::replacements` with the reference and pin already filled in.
type ReplaceFn = dyn Fn(&str, Range<usize>, VersionComment) -> Replacements + Send + Sync;

impl Resolution {
    pub fn new<P: Provider>(reference: P::Ref, pin: P::Pin) -> Self {
        Resolution {
            pin: pin.to_string().into(),
            replacements: Arc::new(move |src, span, comment| {
                P::replacements(&reference, &pin, src, span, comment)
            }),
        }
    }

    /// Edits that pin `occ`, found in `src`.
    pub fn edits(&self, occ: &Occurrence, src: &str) -> Vec<Edit> {
        let comment = occ.ecosystem.version_comment();
        let replacements = (self.replacements)(src, occ.span.clone(), comment);
        to_edits(&occ.path, src, replacements)
    }
}

impl fmt::Display for Resolution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.pin)
    }
}

fn to_edits(path: &Path, src: &str, replacements: Replacements) -> Vec<Edit> {
    replacements
        .into_iter()
        .map(|(span, replacement)| Edit {
            path: path.to_path_buf(),
            expected: src[span.clone()].to_string(),
            span,
            replacement,
        })
        .collect()
}

/// `src` with `P`'s replacements for the reference at `span` applied.
#[cfg(test)]
pub fn pin<P: Provider>(
    reference: &P::Ref,
    to: &P::Pin,
    src: &str,
    span: Range<usize>,
    comment: VersionComment,
) -> String {
    let replacements = P::replacements(reference, to, src, span, comment);
    let edits = to_edits(Path::new("test"), src, replacements);
    crate::domain::edit::apply(src, &edits.iter().collect::<Vec<_>>()).unwrap()
}
