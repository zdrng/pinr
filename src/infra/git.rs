//! Resolves git refs with one `info/refs` request per repository.

use std::collections::BTreeMap;

use anyhow::{Result, bail};

use super::http::{Http, Request};
use crate::domain::model::{GitRef, Resolution};

pub async fn resolve<H: Http>(http: &H, git: &GitRef) -> Result<Resolution> {
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
        let tag = refs
            .iter()
            .filter_map(|(name, s)| Some((name.strip_prefix("refs/tags/")?, s)))
            .filter(|(name, s)| *s == sha && (name == rev || name.starts_with(&format!("{rev}."))))
            .map(|(name, _)| name)
            .max_by_key(|name| name.split('.').count())
            .unwrap_or(rev);
        return Ok(Resolution::Git {
            sha: sha.clone(),
            tag: tag.to_string(),
        });
    }
    if let Some(sha) = refs.get(&format!("refs/heads/{rev}")) {
        return Ok(Resolution::Git {
            sha: sha.clone(),
            tag: rev.clone(),
        });
    }
    bail!("no tag or branch \"{rev}\" in {}", git.repo_url)
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
    use crate::mock::MockHttp;

    const URL: &str = "https://github.com/actions/checkout.git/info/refs?service=git-upload-pack";
    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const C: &str = "cccccccccccccccccccccccccccccccccccccccc";
    const TAG_OBJECT: &str = "dddddddddddddddddddddddddddddddddddddddd";

    /// Build a ref advertisement from `(sha, refname)` lines.
    fn advertisement(refs: &[(&str, &str)]) -> String {
        let pkt = |s: &str| format!("{:04x}{s}", s.len() + 4);
        let mut body = pkt("# service=git-upload-pack\n") + "0000";
        for (i, (sha, name)) in refs.iter().enumerate() {
            let caps = if i == 0 {
                "\0multi_ack side-band-64k"
            } else {
                ""
            };
            body += &pkt(&format!("{sha} {name}{caps}\n"));
        }
        body + "0000"
    }

    async fn resolve_rev(refs: &[(&str, &str)], rev: &str) -> Result<Resolution> {
        let http = MockHttp::new().on(
            Method::Get,
            URL,
            Response::new(200).with_body(advertisement(refs)),
        );
        let git = GitRef {
            repo_url: "https://github.com/actions/checkout".into(),
            rev: rev.into(),
        };
        resolve(&http, &git).await
    }

    fn git(sha: &str, tag: &str) -> Resolution {
        Resolution::Git {
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
        let err = resolve(&http, &git).await.unwrap_err().to_string();
        assert!(err.contains("not found or private (404)"), "{err}");
    }
}
