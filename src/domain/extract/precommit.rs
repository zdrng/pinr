use super::Extractor;
use super::yaml::{Scalar, scalars, sibling};
use crate::domain::model::{Found, GitRef, Kind, Reference};

/// `.pre-commit-config.yaml`.
pub struct PreCommit;

impl Extractor for PreCommit {
    fn extract(&self, src: &str) -> Vec<Found> {
        let scalars = scalars(src);
        scalars
            .iter()
            .filter(|s| matches!(s.path.as_slice(), ["repos", "[]", "repo"]))
            .filter_map(|s| repo(s, &scalars))
            .collect()
    }
}

/// A hook repository with the `rev` next to it. `local` and `meta` hooks are
/// part of the project or pre-commit itself.
fn repo(s: &Scalar, all: &[Scalar]) -> Option<Found> {
    let text = s.text;
    let warning = |msg: String| Some(Found::warning(s.span.clone(), msg));

    if matches!(text, "local" | "meta") {
        return None;
    }
    let Some(url) = text.strip_prefix("https://") else {
        return warning(format!("`{text}` is not an https URL and can't be pinned"));
    };
    let Some(rev) = sibling(all, s, "rev") else {
        return warning(format!("`{text}` has no `rev`"));
    };
    let url = url.trim_end_matches('/');
    let url = url.strip_suffix(".git").unwrap_or(url);
    Some(Found {
        span: rev.span.clone(),
        kind: Kind::Ref(Reference::Git(GitRef {
            repo_url: format!("https://{url}"),
            rev: rev.text.to_string(),
        })),
    })
}

#[cfg(test)]
mod tests {
    use super::super::yaml::summarize;
    use super::*;

    #[test]
    fn hook_repositories() {
        let src = r#"
repos:
  - repo: https://github.com/pre-commit/pre-commit-hooks
    rev: v5.0.0
    hooks:
      - id: trailing-whitespace
  - repo: "https://github.com/psf/black.git"
    rev: '24.10.0'  # frozen: 24.10.0
    hooks: [{id: black}]
  - {repo: https://gitlab.com/org/hooks/, rev: 0161422b4e09b47536ea13f49e786eb3616fe0d7}
  - repo: local
    hooks:
      - id: fmt
        entry: cargo fmt
  - repo: meta
    hooks: [{id: check-useless-excludes}]
  - repo: git@github.com:org/private-hooks
    rev: v1
  - repo: https://github.com/org/no-rev
"#;
        assert_eq!(
            summarize(src, PreCommit.extract(src)),
            [
                (
                    "v5.0.0",
                    "github.com/pre-commit/pre-commit-hooks@v5.0.0".to_string()
                ),
                ("24.10.0", "github.com/psf/black@24.10.0".to_string()),
                (
                    "0161422b4e09b47536ea13f49e786eb3616fe0d7",
                    "gitlab.com/org/hooks@0161422b4e09b47536ea13f49e786eb3616fe0d7".to_string()
                ),
                ("git@github.com:org/private-hooks", "warning".to_string()),
                ("https://github.com/org/no-rev", "warning".to_string()),
            ]
        );
    }
}
