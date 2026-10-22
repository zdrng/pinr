//! Git repositories: tags and branches resolve to commit SHAs, with one
//! `info/refs` request per repository.

use std::collections::BTreeMap;
use std::fmt;
use std::ops::Range;

use anyhow::{Result, bail};

use super::{Provider, Replacements};
use crate::domain::model::GitRef;
use crate::infra::http::{Http, Request};

pub struct Git;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitPin {
    pub sha: String,
    /// The most specific tag on the commit (`v4` -> `v4.2.2`), or the original
    /// revision for branches.
    pub tag: String,
}

impl fmt::Display for GitPin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.sha, self.tag)
    }
}

impl Provider for Git {
    type Ref = GitRef;
    type Pin = GitPin;

    async fn resolve<H: Http>(&self, http: &H, git: &GitRef) -> Result<GitPin> {
        let url = format!("{}.git/info/refs?service=git-upload-pack", git.repo_url);
        let resp = http.send(Request::get(url)).await?;
        if resp.status != 200 {
            bail!(
                "{}: repository not found or private ({})",
                git.repo_url,
                resp.status
            );
        }
        let refs = parse_refs(&resp.body)?;
        let rev = &git.rev;

        if let Some(sha) = refs.get(&format!("refs/tags/{rev}")) {
            // The most specific tag on the same commit: `v4` -> `v4.2.2`.
            let prefix = format!("{rev}.");
            let tag = refs
                .iter()
                .filter_map(|(name, s)| Some((name.strip_prefix("refs/tags/")?, s)))
                .filter(|(name, s)| *s == sha && (name == rev || name.starts_with(&prefix)))
                .map(|(name, _)| name)
                .max_by_key(|name| name.split('.').count())
                .unwrap_or(rev);
            return Ok(GitPin {
                sha: sha.clone(),
                tag: tag.to_string(),
            });
        }
        if let Some(sha) = refs.get(&format!("refs/heads/{rev}")) {
            return Ok(GitPin {
                sha: sha.clone(),
                tag: rev.clone(),
            });
        }
        bail!("no tag or branch \"{rev}\" in {}", git.repo_url)
    }

    /// The SHA plus a `# v4.2.2` comment (git refs only appear in YAML). An
    /// existing version comment is replaced, any other comment is kept.
    fn replacements(git: &GitRef, pin: &GitPin, src: &str, span: Range<usize>) -> Replacements {
        let tag = &pin.tag;
        let eol = src[span.end..]
            .find('\n')
            .map_or(src.len(), |i| span.end + i);
        let rest = src[span.end..eol].trim_end();
        let comment = rest
            .char_indices()
            .find(|&(i, c)| c == '#' && i > 0 && rest[..i].ends_with([' ', '\t']))
            .map(|(i, _)| i);

        let comment = match comment {
            Some(i) if is_version_comment(&rest[i + 1..], &git.rev) => {
                (span.end + i..span.end + rest.len(), format!("# {tag}"))
            }
            Some(i) => {
                let at = span.end + i;
                (at..at, format!("# {tag} "))
            }
            None => {
                let at = span.end + rest.len();
                (at..at, format!(" # {tag}"))
            }
        };
        vec![(span, pin.sha.clone()), comment]
    }
}

/// `# v4`, `# tag=v4.2.2`, `# 1.0` or the old revision itself. A comment with
/// more than one word, like `# v4 needed for tags`, is not.
fn is_version_comment(body: &str, old_rev: &str) -> bool {
    let body = body.trim();
    if body.contains(char::is_whitespace) {
        return false;
    }
    let body = body.strip_prefix("tag=").unwrap_or(body);
    let mut chars = body.chars();
    body == old_rev
        || match chars.next() {
            Some('v') => chars.next().is_some_and(|c| c.is_ascii_digit()),
            Some(c) => c.is_ascii_digit(),
            None => false,
        }
}

/// Parse a smart HTTP ref advertisement into ref name -> commit SHA.
/// Annotated tags map to the commit they point to, not the tag object.
fn parse_refs(body: &[u8]) -> Result<BTreeMap<String, String>> {
    let mut refs = BTreeMap::new();
    let mut rest = body;
    while !rest.is_empty() {
        let len = std::str::from_utf8(rest.get(..4).unwrap_or_default())
            .ok()
            .and_then(|hex| usize::from_str_radix(hex, 16).ok());
        let Some(len) = len else {
            bail!("invalid pkt-line in ref advertisement");
        };
        if len == 0 {
            // Flush packet.
            rest = &rest[4..];
            continue;
        }
        let Some(payload) = rest.get(4..len) else {
            bail!("truncated pkt-line in ref advertisement");
        };
        rest = &rest[len..];

        let line = String::from_utf8_lossy(payload);
        // The first ref line carries capabilities after a NUL byte.
        let line = line.split('\0').next().unwrap_or_default().trim_end();
        let Some((sha, name)) = line.split_once(' ') else {
            continue;
        };
        if sha.starts_with('#') {
            continue;
        }
        // The peeled line follows its tag, so it overwrites the tag object SHA.
        let name = name.strip_suffix("^{}").unwrap_or(name);
        refs.insert(name.to_string(), sha.to_string());
    }
    Ok(refs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::http::{Method, Response};
    use crate::mock::{MockHttp, ref_advertisement};
    use crate::provider::pin;

    const URL: &str = "https://github.com/actions/checkout.git/info/refs?service=git-upload-pack";
    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const C: &str = "cccccccccccccccccccccccccccccccccccccccc";
    const TAG_OBJECT: &str = "dddddddddddddddddddddddddddddddddddddddd";

    async fn resolve_rev(refs: &[(&str, &str)], rev: &str) -> Result<GitPin> {
        let http = MockHttp::new().on(
            Method::Get,
            URL,
            Response::new(200).with_body(ref_advertisement(refs)),
        );
        let git = GitRef {
            repo_url: "https://github.com/actions/checkout".into(),
            rev: rev.into(),
        };
        Git.resolve(&http, &git).await
    }

    fn git(sha: &str, tag: &str) -> GitPin {
        GitPin {
            sha: sha.into(),
            tag: tag.into(),
        }
    }

    #[tokio::test]
    async fn lightweight_tag() {
        let refs = [(C, "HEAD"), (C, "refs/heads/main"), (A, "refs/tags/v1")];
        assert_eq!(resolve_rev(&refs, "v1").await.unwrap(), git(A, "v1"));
    }

    #[tokio::test]
    async fn annotated_tag_uses_peeled_commit() {
        let refs = [
            (C, "HEAD"),
            (TAG_OBJECT, "refs/tags/v1"),
            (A, "refs/tags/v1^{}"),
        ];
        assert_eq!(resolve_rev(&refs, "v1").await.unwrap(), git(A, "v1"));
    }

    #[tokio::test]
    async fn picks_most_specific_tag_on_same_commit() {
        let refs = [
            (C, "HEAD"),
            (A, "refs/tags/v4"),
            (A, "refs/tags/v4.2"),
            (TAG_OBJECT, "refs/tags/v4.2.2"),
            (A, "refs/tags/v4.2.2^{}"),
            (B, "refs/tags/v4.2.3"),
            (A, "refs/tags/v40"),
        ];
        assert_eq!(resolve_rev(&refs, "v4").await.unwrap(), git(A, "v4.2.2"));
    }

    #[tokio::test]
    async fn branch() {
        let refs = [(C, "HEAD"), (B, "refs/heads/main"), (A, "refs/tags/v1")];
        assert_eq!(resolve_rev(&refs, "main").await.unwrap(), git(B, "main"));
    }

    #[tokio::test]
    async fn unknown_rev() {
        let refs = [(C, "HEAD"), (A, "refs/tags/v1")];
        let err = resolve_rev(&refs, "v9").await.unwrap_err().to_string();
        assert_eq!(
            err,
            "no tag or branch \"v9\" in https://github.com/actions/checkout"
        );
    }

    #[tokio::test]
    async fn missing_repo() {
        let http = MockHttp::new().on(Method::Get, URL, Response::new(404));
        let git = GitRef {
            repo_url: "https://github.com/actions/checkout".into(),
            rev: "v1".into(),
        };
        let err = Git.resolve(&http, &git).await.unwrap_err().to_string();
        assert!(err.contains("not found or private (404)"), "{err}");
    }

    fn pin_v4(src: &str) -> String {
        let start = src.find("@v4").unwrap() + 1;
        let git = GitRef {
            repo_url: "https://github.com/actions/checkout".into(),
            rev: "v4".into(),
        };
        let pin_to = GitPin {
            sha: A.into(),
            tag: "v4.2.2".into(),
        };
        pin::<Git>(&git, &pin_to, src, start..start + 2)
    }

    #[test]
    fn gets_sha_and_comment() {
        assert_eq!(
            pin_v4("- uses: actions/checkout@v4\n  with: {}\n"),
            format!("- uses: actions/checkout@{A} # v4.2.2\n  with: {{}}\n")
        );
    }

    #[test]
    fn quoted_and_crlf_lines_are_kept_intact() {
        assert_eq!(
            pin_v4("uses: \"actions/checkout@v4\"  \r\nnext: 1"),
            format!("uses: \"actions/checkout@{A}\" # v4.2.2  \r\nnext: 1")
        );
    }

    #[test]
    fn version_comment_is_replaced() {
        assert_eq!(
            pin_v4("uses: actions/checkout@v4 # v4"),
            format!("uses: actions/checkout@{A} # v4.2.2")
        );
    }

    #[test]
    fn other_comments_are_kept() {
        assert_eq!(
            pin_v4("uses: actions/checkout@v4 # needed for tags"),
            format!("uses: actions/checkout@{A} # v4.2.2 # needed for tags")
        );
        assert_eq!(
            pin_v4("uses: actions/checkout@v4 # v4 needed for tags"),
            format!("uses: actions/checkout@{A} # v4.2.2 # v4 needed for tags")
        );
    }
}
