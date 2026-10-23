use super::Extractor;
use super::image;
use super::yaml::scalars;
use crate::domain::model::Found;

/// `bitbucket-pipelines.yml`.
pub struct BitbucketPipelines;

impl Extractor for BitbucketPipelines {
    fn extract(&self, src: &str) -> Vec<Found> {
        scalars(src)
            .into_iter()
            .filter(|s| {
                matches!(
                    s.path.as_slice(),
                    ["image"]
                        | ["image", "name"]
                        | ["definitions", "services", _, "image"]
                        | ["definitions", "services", _, "image", "name"]
                        // Steps in any pipeline, parallel group, stage or definition.
                        | [.., "step", "image"]
                        | [.., "step", "image", "name"]
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
    fn global_step_and_service_images() {
        let src = r#"
image: atlassian/default-image:4
definitions:
  services:
    db:
      image: postgres:16
    cache:
      image: {name: "redis:7", username: $USER}
  steps:
    - step: &build
        image: rust:1.82
pipelines:
  default:
    - step: *build
    - parallel:
        - step:
            image: {name: node:22}
            script: [npm test]
  branches:
    main:
      - stage:
          steps:
            - step:
                image: $DEPLOY_IMAGE
                script: [./deploy]
                services: [db]
"#;
        assert_eq!(
            summarize(src, BitbucketPipelines.extract(src)),
            [
                (
                    "atlassian/default-image:4",
                    "docker.io/atlassian/default-image:4".to_string()
                ),
                ("postgres:16", "docker.io/library/postgres:16".to_string()),
                ("redis:7", "docker.io/library/redis:7".to_string()),
                ("rust:1.82", "docker.io/library/rust:1.82".to_string()),
                ("node:22", "docker.io/library/node:22".to_string()),
                ("$DEPLOY_IMAGE", "warning".to_string()),
            ]
        );
    }
}
