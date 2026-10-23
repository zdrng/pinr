//! Parsing of reference strings, shared by all extractors.

use crate::domain::model::ImageRef;

/// A full 40-character git commit SHA.
pub fn is_sha(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Parse an image reference like `node:20`, `ghcr.io/org/app:1.2@sha256:…`.
/// Returns a human-readable reason if it can't be pinned.
pub fn parse_image(s: &str) -> Result<ImageRef, String> {
    // `$VAR`, `${VAR}` and CircleCI's `<< parameters.x >>`.
    if s.contains('$') || s.contains("<<") {
        return Err(format!("`{s}` uses a variable and can't be pinned"));
    }
    let (rest, digest) = match s.split_once('@') {
        Some((rest, digest)) => (rest, Some(digest.to_string())),
        None => (s, None),
    };
    let (name, tag) = match rest.rfind(':') {
        Some(i) if !rest[i..].contains('/') => (&rest[..i], Some(rest[i + 1..].to_string())),
        _ => (rest, None),
    };
    let (registry, repository) = match name.split_once('/') {
        // Docker Hub's official images live under `library/`, even when the
        // registry is spelled out: `docker.io/node` is `docker.io/library/node`.
        Some(("docker.io" | "index.docker.io", path)) if !path.contains('/') => {
            ("docker.io".to_string(), format!("library/{path}"))
        }
        Some(("index.docker.io", path)) => ("docker.io".to_string(), path.to_string()),
        Some((first, path)) if first.contains(['.', ':']) || first == "localhost" => {
            (first.to_string(), path.to_string())
        }
        Some(_) => ("docker.io".to_string(), name.to_string()),
        None => ("docker.io".to_string(), format!("library/{name}")),
    };

    let valid_repo = !name.is_empty()
        && !repository.ends_with('/')
        && repository
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-/".contains(&b));
    let valid_tag = tag.as_deref().is_none_or(|t| {
        !t.is_empty()
            && t.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    });
    let valid_digest = digest.as_deref().is_none_or(|d| d.starts_with("sha256:"));
    if !(valid_repo && valid_tag && valid_digest) {
        return Err(format!("`{s}` is not a valid image reference"));
    }

    Ok(ImageRef {
        registry,
        repository,
        tag,
        digest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(
        registry: &str,
        repository: &str,
        tag: Option<&str>,
        digest: Option<&str>,
    ) -> ImageRef {
        ImageRef {
            registry: registry.into(),
            repository: repository.into(),
            tag: tag.map(Into::into),
            digest: digest.map(Into::into),
        }
    }

    #[test]
    fn parses_docker_hub_short_names() {
        assert_eq!(
            parse_image("node").unwrap(),
            image("docker.io", "library/node", None, None)
        );
        assert_eq!(
            parse_image("node:20-alpine").unwrap(),
            image("docker.io", "library/node", Some("20-alpine"), None)
        );
        assert_eq!(
            parse_image("bitnami/redis:7").unwrap(),
            image("docker.io", "bitnami/redis", Some("7"), None)
        );
    }

    #[test]
    fn parses_registries_ports_and_digests() {
        assert_eq!(
            parse_image("ghcr.io/org/app:1.2@sha256:abc").unwrap(),
            image("ghcr.io", "org/app", Some("1.2"), Some("sha256:abc"))
        );
        assert_eq!(
            parse_image("localhost:5000/app").unwrap(),
            image("localhost:5000", "app", None, None)
        );
    }

    #[test]
    fn explicit_docker_hub_names_are_normalized() {
        for s in ["docker.io/node:20", "index.docker.io/library/node:20"] {
            assert_eq!(
                parse_image(s).unwrap(),
                image("docker.io", "library/node", Some("20"), None),
                "{s}"
            );
        }
        assert_eq!(
            parse_image("docker.io/bitnami/redis").unwrap(),
            image("docker.io", "bitnami/redis", None, None)
        );
    }

    #[test]
    fn rejects_variables_and_garbage() {
        assert!(
            parse_image("node:${NODE_VERSION}")
                .unwrap_err()
                .contains("variable")
        );
        assert!(parse_image("$CI_REGISTRY/app").is_err());
        assert!(parse_image("Not An Image").is_err());
        assert!(parse_image("").is_err());
    }

    #[test]
    fn detects_shas() {
        assert!(is_sha("11bd71901bbe5b1630ceea73d27597364c9af683"));
        assert!(!is_sha("v4"));
        assert!(!is_sha("11bd719"));
    }
}
