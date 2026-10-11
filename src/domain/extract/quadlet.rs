//! Podman Quadlet units (`.container`, `.image`, `.volume`): their `Image=` keys.

use super::Extractor;
use crate::domain::model::{Found, Kind, Reference};
use crate::domain::refs::parse_image;

pub struct Quadlet;

impl Extractor for Quadlet {
    fn extract(&self, src: &str) -> Vec<Found> {
        let mut found = Vec::new();
        let mut in_section = false;
        let mut offset = 0;
        for line in src.split_inclusive('\n') {
            let start = offset;
            offset += line.len();
            let line = line.trim_end_matches(['\n', '\r']);
            let trimmed = line.trim();

            if let Some(section) = trimmed.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                in_section = matches!(section, "Container" | "Image" | "Volume");
                continue;
            }
            if !in_section || trimmed.starts_with(['#', ';']) {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim_start();
            let at = start + line.len() - value.len();
            let value = value.trim_end();
            // `foo.image` and `foo.build` refer to other Quadlet units.
            if key.trim() != "Image"
                || value.is_empty()
                || value.ends_with(".image")
                || value.ends_with(".build")
            {
                continue;
            }

            let kind = match parse_image(value) {
                Err(reason) => Kind::Warning(reason),
                Ok(_) if is_short_name(value) => Kind::Warning(short_name_warning(value)),
                Ok(image) => Kind::Ref(Reference::Image(image)),
            };
            found.push(Found {
                span: at..at + value.len(),
                kind,
            });
        }
        found
    }
}

/// An image without a registry, e.g. `nginx` or `library/nginx`.
fn is_short_name(image: &str) -> bool {
    match image.split_once('/') {
        Some((first, _)) => !(first.contains(['.', ':']) || first == "localhost"),
        None => true,
    }
}

fn short_name_warning(image: &str) -> String {
    let suggestion = if image.contains('/') {
        format!("docker.io/{image}")
    } else {
        format!("docker.io/library/{image}")
    };
    format!(
        "`{image}` is a short name that Podman resolves via registries.conf; \
         use a fully qualified name like `{suggestion}`"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extract(src: &str) -> Vec<(&str, Kind)> {
        Quadlet
            .extract(src)
            .into_iter()
            .map(|f| (&src[f.span], f.kind))
            .collect()
    }

    fn img(s: &str) -> Kind {
        Kind::Ref(Reference::Image(parse_image(s).unwrap()))
    }

    #[test]
    fn container_unit() {
        let src = "\
[Unit]
Description=Web
Image=docker.io/ignored:1

[Container]
# Image=docker.io/commented:1
; Image=docker.io/commented:2
Image = docker.io/library/nginx:1.27  \r
ContainerName=web

[Install]
Image=docker.io/ignored:2
";
        assert_eq!(
            extract(src),
            vec![(
                "docker.io/library/nginx:1.27",
                img("docker.io/library/nginx:1.27")
            )]
        );
    }

    #[test]
    fn image_and_volume_units() {
        let src =
            "[Image]\nImage=ghcr.io/org/app:1@sha256:abc\n[Volume]\nImage=localhost/data:latest\n";
        assert_eq!(
            extract(src),
            vec![
                (
                    "ghcr.io/org/app:1@sha256:abc",
                    img("ghcr.io/org/app:1@sha256:abc")
                ),
                ("localhost/data:latest", img("localhost/data:latest")),
            ]
        );
    }

    #[test]
    fn unit_references_are_ignored() {
        let src = "[Container]\nImage=app.image\n[Volume]\nImage=data.build\n";
        assert_eq!(extract(src), vec![]);
    }

    #[test]
    fn short_names_and_variables_are_warned() {
        let src = "[Container]\nImage=nginx:1.27\nImage=bitnami/redis\nImage=quay.io/app:${TAG}\n";
        assert_eq!(
            extract(src),
            vec![
                (
                    "nginx:1.27",
                    Kind::Warning(short_name_warning("nginx:1.27"))
                ),
                (
                    "bitnami/redis",
                    Kind::Warning(short_name_warning("bitnami/redis"))
                ),
                (
                    "quay.io/app:${TAG}",
                    Kind::Warning(parse_image("quay.io/app:${TAG}").unwrap_err())
                ),
            ]
        );
        assert!(short_name_warning("nginx:1.27").contains("docker.io/library/nginx:1.27"));
    }
}
