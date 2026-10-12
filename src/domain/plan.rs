//! Turns a resolved occurrence into text edits. Pure, no I/O.

use crate::domain::model::{Edit, Kind, Occurrence, Reference, Resolution};

/// Edits that pin `occ` (found in `src`) to `res`.
///
/// Git refs get the SHA plus a `# v4.2.2` comment (they only appear in YAML).
/// Images become `name:tag@digest`, which stays readable without a comment.
pub fn plan(occ: &Occurrence, src: &str, res: &Resolution) -> Vec<Edit> {
    let edit = |span: std::ops::Range<usize>, replacement: String| Edit {
        path: occ.path.clone(),
        expected: src[span.clone()].to_string(),
        span,
        replacement,
    };

    match (&occ.kind, res) {
        (Kind::Ref(Reference::Git(git)), Resolution::Git { sha, tag }) => {
            let mut edits = vec![edit(occ.span.clone(), sha.clone())];

            let eol = src[occ.span.end..]
                .find('\n')
                .map_or(src.len(), |i| occ.span.end + i);
            let rest = src[occ.span.end..eol].trim_end();
            let comment = rest
                .char_indices()
                .find(|&(i, c)| c == '#' && i > 0 && rest[..i].ends_with([' ', '\t']))
                .map(|(i, _)| i);

            edits.push(match comment {
                Some(i) if is_version_comment(&rest[i + 1..], &git.rev) => {
                    let start = occ.span.end + i;
                    edit(start..occ.span.end + rest.len(), format!("# {tag}"))
                }
                Some(i) => {
                    let at = occ.span.end + i;
                    edit(at..at, format!("# {tag} "))
                }
                None => {
                    let at = occ.span.end + rest.len();
                    edit(at..at, format!(" # {tag}"))
                }
            });
            edits
        }
        (Kind::Ref(Reference::Image(image)), Resolution::Image { digest }) => {
            let latest = if image.tag.is_none() { ":latest" } else { "" };
            vec![edit(
                occ.span.clone(),
                format!("{}{latest}@{digest}", occ.raw),
            )]
        }
        _ => Vec::new(),
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

/// Apply edits to a file's contents. Fails if the text at an edit's span is not
/// what was planned (the file changed in between).
pub fn apply(src: &str, edits: &[&Edit]) -> Result<String, String> {
    let mut edits = edits.to_vec();
    edits.sort_by_key(|e| std::cmp::Reverse(e.span.start));
    let mut out = src.to_string();
    for e in edits {
        if out.get(e.span.clone()) != Some(e.expected.as_str()) {
            return Err(format!("{} changed since it was scanned", e.path.display()));
        }
        out.replace_range(e.span.clone(), &e.replacement);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model::{Ecosystem, GitRef};
    use crate::domain::refs::parse_image;

    const SHA: &str = "11bd71901bbe5b1630ceea73d27597364c9af683";

    fn git_occ(src: &str, rev: &str) -> Occurrence {
        let start = src.find(&format!("@{rev}")).unwrap() + 1;
        Occurrence {
            path: "ci.yml".into(),
            ecosystem: Ecosystem::GithubActions,
            line: 1,
            span: start..start + rev.len(),
            raw: rev.into(),
            kind: Kind::Ref(Reference::Git(GitRef {
                repo_url: "https://github.com/actions/checkout".into(),
                rev: rev.into(),
            })),
        }
    }

    fn pin_git(src: &str) -> String {
        let occ = git_occ(src, "v4");
        let res = Resolution::Git {
            sha: SHA.into(),
            tag: "v4.2.2".into(),
        };
        let edits = plan(&occ, src, &res);
        apply(src, &edits.iter().collect::<Vec<_>>()).unwrap()
    }

    #[test]
    fn git_ref_gets_sha_and_comment() {
        assert_eq!(
            pin_git("- uses: actions/checkout@v4\n  with: {}\n"),
            format!("- uses: actions/checkout@{SHA} # v4.2.2\n  with: {{}}\n")
        );
    }

    #[test]
    fn quoted_and_crlf_lines_are_kept_intact() {
        assert_eq!(
            pin_git("uses: \"actions/checkout@v4\"  \r\nnext: 1"),
            format!("uses: \"actions/checkout@{SHA}\" # v4.2.2  \r\nnext: 1")
        );
    }

    #[test]
    fn version_comment_is_replaced() {
        assert_eq!(
            pin_git("uses: actions/checkout@v4 # v4"),
            format!("uses: actions/checkout@{SHA} # v4.2.2")
        );
    }

    #[test]
    fn other_comments_are_kept() {
        assert_eq!(
            pin_git("uses: actions/checkout@v4 # needed for tags"),
            format!("uses: actions/checkout@{SHA} # v4.2.2 # needed for tags")
        );
        assert_eq!(
            pin_git("uses: actions/checkout@v4 # v4 needed for tags"),
            format!("uses: actions/checkout@{SHA} # v4.2.2 # v4 needed for tags")
        );
    }

    #[test]
    fn image_gets_digest_and_keeps_tag() {
        for (src, expected) in [
            ("FROM node:20\n", "FROM node:20@sha256:abc\n"),
            (
                "FROM ubuntu AS base\n",
                "FROM ubuntu:latest@sha256:abc AS base\n",
            ),
        ] {
            let raw = src.split_whitespace().nth(1).unwrap();
            let start = src.find(raw).unwrap();
            let occ = Occurrence {
                path: "Dockerfile".into(),
                ecosystem: Ecosystem::Dockerfile,
                line: 1,
                span: start..start + raw.len(),
                raw: raw.into(),
                kind: Kind::Ref(Reference::Image(parse_image(raw).unwrap())),
            };
            let edits = plan(
                &occ,
                src,
                &Resolution::Image {
                    digest: "sha256:abc".into(),
                },
            );
            assert_eq!(
                apply(src, &edits.iter().collect::<Vec<_>>()).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn apply_rejects_stale_edits() {
        let edit = Edit {
            path: "ci.yml".into(),
            span: 0..2,
            expected: "v4".into(),
            replacement: SHA.into(),
        };
        assert!(apply("v5", &[&edit]).is_err());
    }
}
