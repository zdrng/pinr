use super::Extractor;
use super::image;
use super::yaml::{Scalar, scalars};
use crate::domain::model::{Found, GitRef, Kind, Reference};

/// `.gitlab-ci.yml` and included CI files.
pub struct GitlabCi {
    /// Host for `include: project` and `$CI_SERVER_FQDN`, e.g. `gitlab.com`.
    pub host: String,
}

impl Extractor for GitlabCi {
    fn extract(&self, src: &str) -> Vec<Found> {
        let scalars = scalars(src);
        let mut found = Vec::new();
        for s in &scalars {
            match s.path.as_slice() {
                ["include", rest @ ..] => {
                    let rest = rest.strip_prefix(&["[]"]).unwrap_or(rest);
                    found.extend(self.include(s, rest, &scalars));
                }
                path if is_image(path) => found.push(image(s.span.start, s.text)),
                _ => {}
            }
        }
        found
    }
}

/// `image`, `image.name` and `services` at the top level, under `default` or a job.
fn is_image(path: &[&str]) -> bool {
    let rest = match path {
        [first, rest @ ..] if !matches!(*first, "image" | "services" | "variables") => rest,
        _ => path,
    };
    matches!(
        rest,
        ["image"] | ["image", "name"] | ["services", "[]"] | ["services", "[]", "name"]
    )
}

impl GitlabCi {
    /// One value of an `include` entry; `key` is its path inside the entry.
    fn include(&self, s: &Scalar, key: &[&str], all: &[Scalar]) -> Option<Found> {
        let text = s.text;
        let warning = |msg: String| {
            Some(Found {
                span: s.span.clone(),
                kind: Kind::Warning(msg),
            })
        };

        match key {
            [] if text.starts_with("http") => warning(remote(text)),
            ["remote"] => warning(remote(text)),
            ["template"] => warning(format!(
                "template `{text}` comes from the GitLab instance and can't be pinned"
            )),
            ["project"] if text.contains('$') => warning(variable(text)),
            ["project"] => {
                // The `ref` next to `project` in the same mapping.
                let rev = all.iter().find(|r| {
                    r.mapping == s.mapping
                        && r.path.len() == s.path.len()
                        && r.path.last() == Some(&"ref")
                });
                match rev {
                    Some(r) if r.text.contains('$') => warning(variable(r.text)),
                    Some(r) => Some(Found {
                        span: r.span.clone(),
                        kind: Kind::Ref(Reference::Git(GitRef {
                            repo_url: format!("https://{}/{}", self.host, text.trim_matches('/')),
                            rev: r.text.to_string(),
                        })),
                    }),
                    None => warning(format!(
                        "project `{text}` follows the default branch; add a `ref:` to pin it"
                    )),
                }
            }
            ["component"] => self.component(s),
            _ => None,
        }
    }

    /// `<host>/<project path>/<component name>@<version>`.
    fn component(&self, s: &Scalar) -> Option<Found> {
        let text = s.text;
        let warning = |msg: String| {
            Some(Found {
                span: s.span.clone(),
                kind: Kind::Warning(msg),
            })
        };

        let (host, rest) = match text.strip_prefix("$CI_SERVER_FQDN/") {
            Some(rest) => (self.host.as_str(), rest),
            None => text.split_once('/').unwrap_or((text, "")),
        };
        if rest.contains('$') || host.contains('$') {
            return warning(variable(text));
        }
        let Some((path, version)) = rest.rsplit_once('@') else {
            return warning(format!("component `{text}` has no `@` version"));
        };
        if version == "~latest" {
            return warning(format!("component `{text}` follows the latest release"));
        }
        let Some((project, _name)) = path.rsplit_once('/') else {
            return warning(format!("`{text}` is not a valid component reference"));
        };

        let start = s.span.end - version.len();
        Some(Found {
            span: start..s.span.end,
            kind: Kind::Ref(Reference::Git(GitRef {
                repo_url: format!("https://{host}/{project}"),
                rev: version.to_string(),
            })),
        })
    }
}

fn remote(url: &str) -> String {
    format!("remote include `{url}` has no integrity check and can't be pinned")
}

fn variable(text: &str) -> String {
    format!("`{text}` uses a variable and can't be pinned")
}

#[cfg(test)]
mod tests {
    use super::super::yaml::summarize;
    use super::*;

    fn run(src: &str) -> Vec<(&str, String)> {
        let gitlab = GitlabCi {
            host: "gitlab.example.com".to_string(),
        };
        summarize(src, gitlab.extract(src))
    }

    #[test]
    fn images_and_services() {
        let src = r#"
image: "alpine:3.20"
variables:
  image: not-an-image
default:
  image:
    name: ruby:3.3
    entrypoint: [""]
build:
  image: $CI_REGISTRY_IMAGE/builder:latest
  services:
    - postgres:16
    - name: 'redis:7'
      alias: cache
  script: [make]
"#;
        assert_eq!(
            run(src),
            [
                ("alpine:3.20", "docker.io/library/alpine:3.20".to_string()),
                ("ruby:3.3", "docker.io/library/ruby:3.3".to_string()),
                ("$CI_REGISTRY_IMAGE/builder:latest", "warning".to_string()),
                ("postgres:16", "docker.io/library/postgres:16".to_string()),
                ("redis:7", "docker.io/library/redis:7".to_string()),
            ]
        );
    }

    #[test]
    fn include_forms() {
        let src = r#"
include:
  - local: /ci/build.yml
  - ci/test.yml
  - https://example.com/ci.yml
  - remote: 'https://example.com/other.yml'
  - template: Auto-DevOps.gitlab-ci.yml
  - project: my-group/tools
    ref: "v1.2"
    file: [/a.yml, /b.yml]
  - project: my-group/unpinned
    file: /c.yml
  - {project: 'other/repo', ref: main, file: x.yml}
"#;
        assert_eq!(
            run(src),
            [
                ("https://example.com/ci.yml", "warning".to_string()),
                ("https://example.com/other.yml", "warning".to_string()),
                ("Auto-DevOps.gitlab-ci.yml", "warning".to_string()),
                ("v1.2", "gitlab.example.com/my-group/tools@v1.2".to_string()),
                ("my-group/unpinned", "warning".to_string()),
                ("main", "gitlab.example.com/other/repo@main".to_string()),
            ]
        );
    }

    #[test]
    fn single_include_and_components() {
        let src = r#"
include:
  - component: $CI_SERVER_FQDN/my-org/security/secret-detection@1.0.2
    inputs:
      stage: test
  - component: gitlab.com/components/opentofu/full-pipeline@~latest
  - component: "gitlab.com/a/b/c@11bd71901bbe5b1630ceea73d27597364c9af683"
"#;
        assert_eq!(
            run(src),
            [
                (
                    "1.0.2",
                    "gitlab.example.com/my-org/security@1.0.2".to_string()
                ),
                (
                    "gitlab.com/components/opentofu/full-pipeline@~latest",
                    "warning".to_string()
                ),
                (
                    "11bd71901bbe5b1630ceea73d27597364c9af683",
                    "gitlab.com/a/b@11bd71901bbe5b1630ceea73d27597364c9af683".to_string()
                ),
            ]
        );

        let src = "include:\n  project: g/p\n  ref: v2\n";
        assert_eq!(run(src), [("v2", "gitlab.example.com/g/p@v2".to_string())]);
        assert_eq!(run("include: '/local.yml'\n"), []);
    }
}
