//! Container images: tags resolve to digests via the OCI distribution API.

use std::fmt;
use std::ops::Range;

use anyhow::{Context, Result, bail};

use super::{Provider, Replacements};
use crate::domain::model::ImageRef;
use crate::infra::http::{Http, Request, Response};

/// Index types first, so multi-arch images pin the index rather than one platform.
const ACCEPT: &str = "application/vnd.oci.image.index.v1+json, \
    application/vnd.docker.distribution.manifest.list.v2+json, \
    application/vnd.docker.distribution.manifest.v2+json, \
    application/vnd.oci.image.manifest.v1+json";

pub struct Oci;

/// A manifest digest, e.g. `sha256:…`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Digest(pub String);

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Provider for Oci {
    type Ref = ImageRef;
    type Pin = Digest;

    async fn resolve<H: Http>(&self, http: &H, image: &ImageRef) -> Result<Digest> {
        let host = match image.registry.as_str() {
            "docker.io" => "registry-1.docker.io",
            other => other,
        };
        let tag = image.tag.as_deref().unwrap_or("latest");
        let name = format!("{}/{}:{tag}", image.registry, image.repository);
        let url = format!("https://{host}/v2/{}/manifests/{tag}", image.repository);

        // HEAD doesn't count against Docker Hub pull limits.
        let head = || Request::head(&url).header("Accept", ACCEPT);
        let mut resp = http.send(head()).await?;
        if resp.status == 401 {
            let token = anonymous_token(http, &resp)
                .await
                .with_context(|| format!("{name}: authentication failed"))?;
            let req = head().header("Authorization", format!("Bearer {token}"));
            resp = http.send(req).await?;
        }

        match resp.status {
            200 => {}
            404 => bail!("{name}: manifest unknown (404)"),
            401 | 403 => bail!("{name}: access denied ({})", resp.status),
            status => bail!("{name}: unexpected response ({status})"),
        }
        let digest = resp
            .header("Docker-Content-Digest")
            .with_context(|| format!("{name}: registry sent no Docker-Content-Digest"))?;
        Ok(Digest(digest.to_string()))
    }

    /// `name:tag@digest`, which stays readable without a comment.
    fn replacements(
        image: &ImageRef,
        digest: &Digest,
        src: &str,
        span: Range<usize>,
    ) -> Replacements {
        let latest = if image.tag.is_none() { ":latest" } else { "" };
        let pinned = format!("{}{latest}@{digest}", &src[span.clone()]);
        vec![(span, pinned)]
    }
}

/// Fetch an anonymous pull token from the realm in a 401's `WWW-Authenticate`.
async fn anonymous_token<H: Http>(http: &H, challenge: &Response) -> Result<String> {
    let header = challenge
        .header("WWW-Authenticate")
        .context("no WWW-Authenticate header")?;
    let params = header
        .strip_prefix("Bearer ")
        .context("not a Bearer challenge")?;
    let params = parse_params(params);
    let get = |key: &str| params.iter().find(|(k, _)| k == key).map(|(_, v)| v);

    let mut url = get("realm").context("no realm in challenge")?.clone();
    let query: Vec<String> = ["service", "scope"]
        .iter()
        .filter_map(|key| Some(format!("{key}={}", get(key)?)))
        .collect();
    if !query.is_empty() {
        url = format!("{url}?{}", query.join("&"));
    }

    let resp = http.send(Request::get(url)).await?;
    if resp.status != 200 {
        bail!("token endpoint returned {}", resp.status);
    }
    let json: serde_json::Value = serde_json::from_slice(&resp.body)?;
    json.get("token")
        .or_else(|| json.get("access_token"))
        .and_then(|t| t.as_str())
        .map(str::to_string)
        .context("no token in response")
}

/// Parse `realm="…",service="…"`. Quoted values may contain commas.
fn parse_params(s: &str) -> Vec<(String, String)> {
    let mut params = Vec::new();
    let mut rest = s;
    while let Some((key, after)) = rest.split_once('=') {
        let key = key.trim_matches([',', ' ']).to_string();
        let (value, next) = match after.strip_prefix('"') {
            Some(quoted) => quoted.split_once('"').unwrap_or((quoted, "")),
            None => after.split_once(',').unwrap_or((after, "")),
        };
        params.push((key, value.to_string()));
        rest = next;
    }
    params
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::refs::parse_image;
    use crate::infra::http::Method;
    use crate::mock::MockHttp;
    use crate::provider::pin;

    const DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn image(registry: &str, repository: &str, tag: Option<&str>) -> ImageRef {
        ImageRef {
            registry: registry.into(),
            repository: repository.into(),
            tag: tag.map(Into::into),
            digest: None,
        }
    }

    fn found() -> Response {
        Response::new(200).with_header("Docker-Content-Digest", DIGEST)
    }

    fn pinned() -> Digest {
        Digest(DIGEST.into())
    }

    #[tokio::test]
    async fn direct_200() {
        let http = MockHttp::new().on(
            Method::Head,
            "https://ghcr.io/v2/org/app/manifests/1.2",
            found(),
        );
        let res = Oci
            .resolve(&http, &image("ghcr.io", "org/app", Some("1.2")))
            .await;
        assert_eq!(res.unwrap(), pinned());
        let calls = http.calls();
        let accept = &calls[0].headers[0];
        assert_eq!(accept.0, "Accept");
        assert!(
            accept
                .1
                .starts_with("application/vnd.oci.image.index.v1+json")
        );
    }

    #[tokio::test]
    async fn docker_hub_token_flow_and_latest() {
        let manifest = "https://registry-1.docker.io/v2/library/node/manifests/latest";
        let challenge = Response::new(401).with_header(
            "www-authenticate",
            r#"Bearer realm="https://auth.docker.io/token",service="registry.docker.io",scope="repository:library/node:pull""#,
        );
        let token = Response::new(200).with_body(r#"{"token":"abc","expires_in":300}"#);
        let http = MockHttp::new()
            .on(Method::Head, manifest, challenge)
            .on(Method::Head, manifest, found())
            .on(
                Method::Get,
                "https://auth.docker.io/token?service=registry.docker.io&scope=repository:library/node:pull",
                token,
            );
        let res = Oci
            .resolve(&http, &image("docker.io", "library/node", None))
            .await;
        assert_eq!(res.unwrap(), pinned());

        let calls = http.calls();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[2].method, Method::Head);
        assert_eq!(calls[2].url, manifest);
        let auth = ("Authorization".to_string(), "Bearer abc".to_string());
        assert!(calls[2].headers.contains(&auth));
    }

    #[tokio::test]
    async fn access_token_field() {
        let manifest = "https://ghcr.io/v2/org/app/manifests/1";
        let challenge = Response::new(401).with_header(
            "WWW-Authenticate",
            r#"Bearer realm="https://ghcr.io/token",scope="repository:org/app:pull""#,
        );
        let http = MockHttp::new()
            .on(Method::Head, manifest, challenge)
            .on(Method::Head, manifest, found())
            .on(
                Method::Get,
                "https://ghcr.io/token?scope=repository:org/app:pull",
                Response::new(200).with_body(r#"{"access_token":"xyz"}"#),
            );
        let res = Oci
            .resolve(&http, &image("ghcr.io", "org/app", Some("1")))
            .await;
        assert_eq!(res.unwrap(), pinned());
        let auth = ("Authorization".to_string(), "Bearer xyz".to_string());
        assert!(http.calls()[2].headers.contains(&auth));
    }

    #[tokio::test]
    async fn manifest_unknown() {
        let http = MockHttp::new().on(
            Method::Head,
            "https://registry-1.docker.io/v2/library/node/manifests/20",
            Response::new(404),
        );
        let err = Oci
            .resolve(&http, &image("docker.io", "library/node", Some("20")))
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "docker.io/library/node:20: manifest unknown (404)"
        );
    }

    #[tokio::test]
    async fn missing_digest_header() {
        let http = MockHttp::new().on(
            Method::Head,
            "https://quay.io/v2/org/app/manifests/1",
            Response::new(200),
        );
        let err = Oci
            .resolve(&http, &image("quay.io", "org/app", Some("1")))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Docker-Content-Digest"), "{err}");
    }

    #[test]
    fn params_with_commas_in_quotes() {
        let params = parse_params(r#"realm="https://x/token",scope="repository:a:pull,push""#);
        assert_eq!(
            params,
            [
                ("realm".to_string(), "https://x/token".to_string()),
                ("scope".to_string(), "repository:a:pull,push".to_string()),
            ]
        );
    }

    #[test]
    fn gets_digest_and_keeps_tag() {
        for (src, expected) in [
            ("FROM node:20\n", "FROM node:20@sha256:abc\n"),
            (
                "FROM ubuntu AS base\n",
                "FROM ubuntu:latest@sha256:abc AS base\n",
            ),
        ] {
            let raw = src.split_whitespace().nth(1).unwrap();
            let start = src.find(raw).unwrap();
            let image = parse_image(raw).unwrap();
            let digest = Digest("sha256:abc".into());
            let span = start..start + raw.len();
            assert_eq!(pin::<Oci>(&image, &digest, src, span), expected);
        }
    }
}
