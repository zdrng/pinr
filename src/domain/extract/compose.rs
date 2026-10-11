use super::Extractor;
use super::image;
use super::yaml::scalars;
use crate::domain::model::Found;

/// Docker Compose files.
pub struct Compose;

impl Extractor for Compose {
    fn extract(&self, src: &str) -> Vec<Found> {
        scalars(src)
            .into_iter()
            .filter(|s| matches!(s.path.as_slice(), ["services", _, "image"]))
            .map(|s| image(s.span.start, s.text))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::super::yaml::summarize;
    use super::*;

    #[test]
    fn service_images() {
        let src = r#"
services:
  web:
    image: "nginx:1.27"
    ports: ["80:80"]
  db: {image: postgres@sha256:abc}
  app:
    image: ${IMAGE}
  built:
    build: .
volumes:
  data:
    image: not-a-service
"#;
        assert_eq!(
            summarize(src, Compose.extract(src)),
            [
                ("nginx:1.27", "docker.io/library/nginx:1.27".to_string()),
                (
                    "postgres@sha256:abc",
                    "docker.io/library/postgres@sha256:abc".to_string()
                ),
                ("${IMAGE}", "warning".to_string()),
            ]
        );
    }
}
