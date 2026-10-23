use super::Extractor;
use super::image;
use super::yaml::scalars;
use crate::domain::model::Found;

/// Woodpecker (`.woodpecker.yaml`, `.woodpecker/*.yaml`) and Drone (`.drone.yml`)
/// pipelines, which share their syntax for images. Plugins are images, too.
pub struct Woodpecker;

impl Extractor for Woodpecker {
    fn extract(&self, src: &str) -> Vec<Found> {
        scalars(src)
            .into_iter()
            // Steps and services as a list or as a map by name. `pipeline` is
            // the old name of `steps`.
            .filter(|s| {
                matches!(
                    s.path.as_slice(),
                    ["steps" | "services" | "clone" | "pipeline", _, "image"]
                )
            })
            .map(|s| image(s.span.start, s.text))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::super::yaml::summarize;
    use super::*;

    #[test]
    fn steps_services_and_clone() {
        let src = r#"
clone:
  git:
    image: woodpeckerci/plugin-git:2.6
steps:
  - name: build
    image: golang:1.23
    commands: [go build]
  - name: publish
    image: woodpeckerci/plugin-docker-buildx
    settings:
      image: not/an-image
services:
  - name: database
    image: "mysql:8"
---
kind: pipeline
steps:
  test:
    image: ${CI_IMAGE}
"#;
        assert_eq!(
            summarize(src, Woodpecker.extract(src)),
            [
                (
                    "woodpeckerci/plugin-git:2.6",
                    "docker.io/woodpeckerci/plugin-git:2.6".to_string()
                ),
                ("golang:1.23", "docker.io/library/golang:1.23".to_string()),
                (
                    "woodpeckerci/plugin-docker-buildx",
                    "docker.io/woodpeckerci/plugin-docker-buildx".to_string()
                ),
                ("mysql:8", "docker.io/library/mysql:8".to_string()),
                ("${CI_IMAGE}", "warning".to_string()),
            ]
        );
    }
}
