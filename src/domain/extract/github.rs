use super::Extractor;
use super::image;
use super::yaml::{Scalar, scalars};
use crate::domain::model::{Found, GitRef, Kind, Reference};

/// Workflows and composite actions (`action.yml`) for GitHub Actions and the
/// compatible Forgejo and Gitea Actions.
pub struct Actions {
    /// Where `uses: owner/repo@rev` is fetched from, e.g. `https://github.com`.
    pub default_url: String,
}

impl Extractor for Actions {
    fn extract(&self, src: &str) -> Vec<Found> {
        let mut found = Vec::new();
        for s in scalars(src) {
            match s.path.as_slice() {
                ["jobs", _, "steps", "[]", "uses"]
                | ["runs", "steps", "[]", "uses"]
                | ["jobs", _, "uses"] => found.extend(self.uses(&s)),
                ["jobs", _, "container"]
                | ["jobs", _, "container", "image"]
                | ["jobs", _, "services", _, "image"] => found.push(image(s.span.start, s.text)),
                _ => {}
            }
        }
        found
    }
}

impl Actions {
    /// `owner/repo[/path]@rev`, `https://host/owner/repo[/path]@rev`,
    /// `docker://image` or a local `./path`.
    fn uses(&self, s: &Scalar) -> Option<Found> {
        let warning = |msg: String| Some(Found::warning(s.span.clone(), msg));
        let text = s.text;

        if text.starts_with("./") {
            return None;
        }
        if text.contains("${{") {
            return warning(format!("`{text}` uses an expression and can't be pinned"));
        }
        if let Some(name) = text.strip_prefix("docker://") {
            return Some(image(s.span.start + "docker://".len(), name));
        }
        let Some((name, rev)) = text.split_once('@') else {
            return warning(format!("`{text}` has no `@` revision"));
        };
        let (base, name) = match name.strip_prefix("https://") {
            Some(url) => {
                let Some((host, name)) = url.split_once('/') else {
                    return warning(format!("`{text}` is not a valid action reference"));
                };
                (format!("https://{host}"), name)
            }
            None => (self.default_url.trim_end_matches('/').to_string(), name),
        };
        let mut parts = name.split('/');
        let (Some(owner), Some(repo)) = (parts.next(), parts.next()) else {
            return warning(format!("`{text}` is not a valid action reference"));
        };
        if owner.is_empty() || repo.is_empty() || rev.is_empty() {
            return warning(format!("`{text}` is not a valid action reference"));
        }

        Some(Found {
            span: s.span.end - rev.len()..s.span.end,
            kind: Kind::Ref(Reference::Git(GitRef {
                repo_url: format!("{base}/{owner}/{repo}"),
                rev: rev.to_string(),
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::yaml::summarize;
    use super::*;

    fn run(src: &str) -> Vec<(&str, String)> {
        let github = Actions {
            default_url: "https://github.com".into(),
        };
        summarize(src, github.extract(src))
    }

    #[test]
    fn steps_and_quoted_uses() {
        let src = r#"
on: push
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Setup
        uses: "actions/setup-node@v4.1.0"  # keep
        with:
          node-version: 20
      - uses: 'github/codeql-action/init@11bd71901bbe5b1630ceea73d27597364c9af683'
      - run: echo hi
"#;
        assert_eq!(
            run(src),
            [
                ("v4", "github.com/actions/checkout@v4".to_string()),
                ("v4.1.0", "github.com/actions/setup-node@v4.1.0".to_string()),
                (
                    "11bd71901bbe5b1630ceea73d27597364c9af683",
                    "github.com/github/codeql-action@11bd71901bbe5b1630ceea73d27597364c9af683"
                        .to_string()
                ),
            ]
        );
    }

    #[test]
    fn local_docker_and_reusable_workflows() {
        let src = r#"
jobs:
  call:
    uses: octo-org/ci/.github/workflows/build.yml@main
  local-call:
    uses: ./.github/workflows/local.yml
  test:
    steps:
      - uses: ./.github/actions/setup
      - uses: docker://alpine:3.20
      - {uses: "actions/cache@v4"}
"#;
        assert_eq!(
            run(src),
            [
                ("main", "github.com/octo-org/ci@main".to_string()),
                ("alpine:3.20", "docker.io/library/alpine:3.20".to_string()),
                ("v4", "github.com/actions/cache@v4".to_string()),
            ]
        );
    }

    #[test]
    fn containers_and_services() {
        let src = r#"
jobs:
  a:
    container: node:20
    services:
      db:
        image: "postgres:16"
  b:
    container:
      image: ghcr.io/org/app:1.2
      options: --cpus 1
    services:
      redis: { image: '${{ matrix.redis }}' }
"#;
        assert_eq!(
            run(src),
            [
                ("node:20", "docker.io/library/node:20".to_string()),
                ("postgres:16", "docker.io/library/postgres:16".to_string()),
                ("ghcr.io/org/app:1.2", "ghcr.io/org/app:1.2".to_string()),
                ("${{ matrix.redis }}", "warning".to_string()),
            ]
        );
    }

    #[test]
    fn composite_actions_and_warnings() {
        let src = r#"
name: My action
runs:
  using: composite
  steps:
    - uses: actions/checkout@v4
    - uses: actions/checkout
    - uses: ${{ inputs.action }}
"#;
        assert_eq!(
            run(src),
            [
                ("v4", "github.com/actions/checkout@v4".to_string()),
                ("actions/checkout", "warning".to_string()),
                ("${{ inputs.action }}", "warning".to_string()),
            ]
        );
    }

    #[test]
    fn default_url_and_absolute_urls() {
        let src = r#"
jobs:
  build:
    steps:
      - uses: actions/checkout@v4
      - uses: https://code.forgejo.org/actions/setup-go@v5
      - uses: https://github.com/actions/cache/restore@v4
      - uses: https://example.com@v1
"#;
        let forgejo = Actions {
            default_url: "https://data.forgejo.org/".into(),
        };
        assert_eq!(
            summarize(src, forgejo.extract(src)),
            [
                ("v4", "data.forgejo.org/actions/checkout@v4".to_string()),
                ("v5", "code.forgejo.org/actions/setup-go@v5".to_string()),
                ("v4", "github.com/actions/cache@v4".to_string()),
                ("https://example.com@v1", "warning".to_string()),
            ]
        );
    }
}
