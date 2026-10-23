use super::Extractor;
use super::image;
use super::yaml::{Scalar, scalars};
use crate::domain::model::Found;

/// `.circleci/config.yml`.
pub struct CircleCi;

impl Extractor for CircleCi {
    fn extract(&self, src: &str) -> Vec<Found> {
        let mut found = Vec::new();
        for s in scalars(src) {
            match s.path.as_slice() {
                // Jobs, executors and inline orbs.
                [.., "docker", "[]", "image"] => found.push(image(s.span.start, s.text)),
                ["orbs", _] => found.extend(orb(&s)),
                _ => {}
            }
        }
        found
    }
}

/// `circleci/node@5.1.0`. Published orb versions can't change, but a partial
/// version like `@5`, `@volatile` and dev versions follow new releases.
fn orb(s: &Scalar) -> Option<Found> {
    let version = s.text.rsplit_once('@').map_or("", |(_, v)| v);
    let parts: Vec<&str> = version.split('.').collect();
    let full = parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    (!full).then(|| {
        let text = s.text;
        Found::warning(
            s.span.clone(),
            format!("orb `{text}` follows new releases; use a full version like `@1.2.3`"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::super::yaml::summarize;
    use super::*;

    #[test]
    fn docker_images_and_orbs() {
        let src = r#"
version: 2.1
orbs:
  node: circleci/node@5.1.0
  aws: circleci/aws-cli@4
  slack: circleci/slack@volatile
  local:
    executors:
      tiny:
        docker: [{image: busybox:1.36}]
executors:
  rust:
    docker:
      - image: cimg/rust:1.82
jobs:
  test:
    docker:
      - image: "cimg/node:22.11"
        auth: {username: me}
      - image: postgres:16@sha256:abc
      - image: cimg/base:<< parameters.tag >>
    steps: [checkout]
"#;
        assert_eq!(
            summarize(src, CircleCi.extract(src)),
            [
                ("circleci/aws-cli@4", "warning".to_string()),
                ("circleci/slack@volatile", "warning".to_string()),
                ("busybox:1.36", "docker.io/library/busybox:1.36".to_string()),
                ("cimg/rust:1.82", "docker.io/cimg/rust:1.82".to_string()),
                ("cimg/node:22.11", "docker.io/cimg/node:22.11".to_string()),
                (
                    "postgres:16@sha256:abc",
                    "docker.io/library/postgres:16@sha256:abc".to_string()
                ),
                ("cimg/base:<< parameters.tag >>", "warning".to_string()),
            ]
        );
    }
}
