//! End-to-end through the service: file contents come from `MockFs`, registry
//! and git responses from `MockHttp`.

use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::discover::detect;
use crate::domain::model::{Kind, Occurrence};
use crate::infra::http::{Method, Response};
use crate::mock::{MockFs, MockHttp, ref_advertisement};
use crate::service::{PinService, unpinned};

const CHECKOUT_SHA: &str = "11bd71901bbe5b1630ceea73d27597364c9af683";
const SETUP_NODE_TAG_OBJECT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SETUP_NODE_SHA: &str = "49933ea5288caeca8642d1e84afbd3f7d6820020";
const TEMPLATES_SHA: &str = "cccccccccccccccccccccccccccccccccccccccc";
const CACHE_SHA: &str = "0400d5f644dc74513175e3cd8d07132dd4860809";

const CI_YML: &str = "\
name: ci
on: push
jobs:
  build:
    runs-on: ubuntu-latest
    container: node:20
    steps:
      - uses: actions/checkout@v4
      - uses: ./.github/actions/local
      - uses: actions/setup-node@v4 # v4
      - uses: actions/cache@0400d5f644dc74513175e3cd8d07132dd4860809 # v4.2.4
";

const RELEASE_YML: &str = "\
jobs:
  release:
    runs-on: ubuntu-latest
    steps:
      - uses: \"actions/checkout@v4\"
";

const DOCKERFILE: &str = "\
FROM node:20 AS build
RUN npm ci
FROM gcr.io/distroless/nodejs20-debian12
COPY --from=build /app /app
";

const COMPOSE: &str = "\
services:
  db:
    image: quay.io/acme/postgres:16
  app:
    image: ${APP_IMAGE}
";

const GITLAB_CI: &str = "\
include:
  - project: group/templates
    ref: v1.2
    file: /ci.yml
test:
  image: ghcr.io/acme/runner:1
  script: [make test]
";

const QUADLET: &str = "\
[Container]
Image=ghcr.io/acme/web:2.0
";

fn digest(c: char) -> String {
    format!("sha256:{}", c.to_string().repeat(64))
}

fn git_mock(http: MockHttp, repo_url: &str, refs: &[(&str, &str)]) -> MockHttp {
    let url = format!("{repo_url}.git/info/refs?service=git-upload-pack");
    http.on(
        Method::Get,
        &url,
        Response::new(200).with_body(ref_advertisement(refs)),
    )
}

fn image_mock(http: MockHttp, manifest_url: &str, digest: &str) -> MockHttp {
    let ok = Response::new(200).with_header("Docker-Content-Digest", digest);
    http.on(Method::Head, manifest_url, ok)
}

fn mocked_http() -> MockHttp {
    let http = MockHttp::new();
    let http = git_mock(
        http,
        "https://github.com/actions/checkout",
        &[
            (CHECKOUT_SHA, "HEAD"),
            (CHECKOUT_SHA, "refs/heads/main"),
            (CHECKOUT_SHA, "refs/tags/v4"),
            (CHECKOUT_SHA, "refs/tags/v4.2"),
            (CHECKOUT_SHA, "refs/tags/v4.2.2"),
            ("dddddddddddddddddddddddddddddddddddddddd", "refs/tags/v3"),
        ],
    );
    // Annotated tags: the peeled `^{}` line holds the commit.
    let http = git_mock(
        http,
        "https://github.com/actions/setup-node",
        &[
            (SETUP_NODE_SHA, "HEAD"),
            (SETUP_NODE_TAG_OBJECT, "refs/tags/v4"),
            (SETUP_NODE_SHA, "refs/tags/v4^{}"),
            (
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "refs/tags/v4.4.0",
            ),
            (SETUP_NODE_SHA, "refs/tags/v4.4.0^{}"),
        ],
    );
    let http = git_mock(
        http,
        "https://gitlab.com/group/templates",
        &[(TEMPLATES_SHA, "refs/tags/v1.2")],
    );

    // Docker Hub wants an anonymous token first.
    let node = "https://registry-1.docker.io/v2/library/node/manifests/20";
    let challenge = "Bearer realm=\"https://auth.docker.io/token\",\
                     service=\"registry.docker.io\",scope=\"repository:library/node:pull\"";
    let http = http
        .on(
            Method::Head,
            node,
            Response::new(401).with_header("WWW-Authenticate", challenge),
        )
        .on(
            Method::Get,
            "https://auth.docker.io/token?service=registry.docker.io&scope=repository:library/node:pull",
            Response::new(200).with_body(r#"{"token":"anon-token"}"#),
        );
    let http = image_mock(http, node, &digest('a'));

    let http = image_mock(
        http,
        "https://gcr.io/v2/distroless/nodejs20-debian12/manifests/latest",
        &digest('b'),
    );
    let http = image_mock(
        http,
        "https://quay.io/v2/acme/postgres/manifests/16",
        &digest('c'),
    );
    let http = image_mock(
        http,
        "https://ghcr.io/v2/acme/runner/manifests/1",
        &digest('d'),
    );
    image_mock(
        http,
        "https://ghcr.io/v2/acme/web/manifests/2.0",
        &digest('e'),
    )
}

const FILES: [(&str, &str); 6] = [
    (".github/workflows/ci.yml", CI_YML),
    (".github/workflows/release.yml", RELEASE_YML),
    ("Dockerfile", DOCKERFILE),
    ("compose.yaml", COMPOSE),
    (".gitlab-ci.yml", GITLAB_CI),
    ("deploy/web.container", QUADLET),
];

fn service() -> (PinService<MockFs, MockHttp>, Vec<Occurrence>) {
    let service = PinService::new(MockFs::new(&FILES), mocked_http(), Config::default());
    let occurrences = service.scan(&files());
    (service, occurrences)
}

/// What `discover` would return for `FILES`.
fn files() -> Vec<(PathBuf, crate::domain::model::Ecosystem)> {
    FILES
        .iter()
        .map(|(path, _)| (PathBuf::from(path), detect(path.as_ref()).unwrap()))
        .collect()
}

fn warnings(occurrences: &[Occurrence]) -> Vec<String> {
    occurrences
        .iter()
        .filter(|o| matches!(o.kind, Kind::Warning(_)))
        .map(|o| format!("{}:{} {}", o.path.display(), o.line, o.raw))
        .collect()
}

#[test]
fn scan_finds_unpinned_refs_and_warnings() {
    let (_, occurrences) = service();

    let mut found: Vec<String> = unpinned(&occurrences)
        .map(|o| format!("{}:{} {}", o.path.display(), o.line, o.raw))
        .collect();
    found.sort();
    assert_eq!(
        found,
        [
            ".github/workflows/ci.yml:10 v4",
            ".github/workflows/ci.yml:6 node:20",
            ".github/workflows/ci.yml:8 v4",
            ".github/workflows/release.yml:5 v4",
            ".gitlab-ci.yml:3 v1.2",
            ".gitlab-ci.yml:6 ghcr.io/acme/runner:1",
            "Dockerfile:1 node:20",
            "Dockerfile:3 gcr.io/distroless/nodejs20-debian12",
            "compose.yaml:3 quay.io/acme/postgres:16",
            "deploy/web.container:2 ghcr.io/acme/web:2.0",
        ]
    );
    assert_eq!(warnings(&occurrences), ["compose.yaml:5 ${APP_IMAGE}"]);
}

#[tokio::test]
async fn pin_rewrites_every_file() {
    let (service, occurrences) = service();

    let resolutions = service.resolve_all(&occurrences).await;
    let targets: Vec<&Occurrence> = unpinned(&occurrences).collect();
    let edits = service.plan(&targets, &resolutions).unwrap();
    assert_eq!(service.apply(&edits).unwrap(), 6);

    let fs = service.fs();
    assert_eq!(
        fs.get(".github/workflows/ci.yml"),
        format!(
            "\
name: ci
on: push
jobs:
  build:
    runs-on: ubuntu-latest
    container: node:20@{node}
    steps:
      - uses: actions/checkout@{CHECKOUT_SHA} # v4.2.2
      - uses: ./.github/actions/local
      - uses: actions/setup-node@{SETUP_NODE_SHA} # v4.4.0
      - uses: actions/cache@{CACHE_SHA} # v4.2.4
",
            node = digest('a')
        )
    );
    assert_eq!(
        fs.get(".github/workflows/release.yml"),
        format!(
            "\
jobs:
  release:
    runs-on: ubuntu-latest
    steps:
      - uses: \"actions/checkout@{CHECKOUT_SHA}\" # v4.2.2
"
        )
    );
    assert_eq!(
        fs.get("Dockerfile"),
        format!(
            "\
FROM node:20@{} AS build
RUN npm ci
FROM gcr.io/distroless/nodejs20-debian12:latest@{}
COPY --from=build /app /app
",
            digest('a'),
            digest('b')
        )
    );
    assert_eq!(
        fs.get("compose.yaml"),
        format!(
            "\
services:
  db:
    image: quay.io/acme/postgres:16@{}
  app:
    image: ${{APP_IMAGE}}
",
            digest('c')
        )
    );
    assert_eq!(
        fs.get(".gitlab-ci.yml"),
        format!(
            "\
include:
  - project: group/templates
    ref: {TEMPLATES_SHA} # v1.2
    file: /ci.yml
test:
  image: ghcr.io/acme/runner:1@{}
  script: [make test]
",
            digest('d')
        )
    );
    assert_eq!(
        fs.get("deploy/web.container"),
        format!("[Container]\nImage=ghcr.io/acme/web:2.0@{}\n", digest('e'))
    );

    // Pinning is idempotent: a rescan finds nothing left to do.
    let rescanned = service.scan(&files());
    assert_eq!(unpinned(&rescanned).count(), 0);
}

#[tokio::test]
async fn each_reference_is_resolved_once() {
    let (service, occurrences) = service();
    service.resolve_all(&occurrences).await;

    let calls = service.http().calls();
    let count = |url: &str| calls.iter().filter(|r| r.url == url).count();
    // actions/checkout@v4 appears twice, node:20 in a workflow and a Dockerfile.
    assert_eq!(
        count("https://github.com/actions/checkout.git/info/refs?service=git-upload-pack"),
        1
    );
    // 401, then the retry with the token.
    assert_eq!(
        count("https://registry-1.docker.io/v2/library/node/manifests/20"),
        2
    );
    let retry = calls
        .iter()
        .rfind(|r| r.url.ends_with("library/node/manifests/20"))
        .unwrap();
    assert!(
        retry
            .headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("authorization") && v == "Bearer anon-token")
    );
}

#[tokio::test]
async fn failed_resolutions_leave_files_untouched() {
    let fs = MockFs::new(&[("Dockerfile", "FROM ghcr.io/acme/missing:1\n")]);
    let http = MockHttp::new().on(
        Method::Head,
        "https://ghcr.io/v2/acme/missing/manifests/1",
        Response::new(404),
    );
    let service = PinService::new(fs, http, Config::default());
    let occurrences = service.scan(&[(
        "Dockerfile".into(),
        crate::domain::model::Ecosystem::Dockerfile,
    )]);

    let resolutions = service.resolve_all(&occurrences).await;
    assert_eq!(resolutions.len(), 1);
    assert!(resolutions.values().all(|r| r.is_err()));

    let targets: Vec<&Occurrence> = unpinned(&occurrences).collect();
    let edits = service.plan(&targets, &resolutions).unwrap();
    assert!(edits.is_empty());
    assert_eq!(
        service.fs().get("Dockerfile"),
        "FROM ghcr.io/acme/missing:1\n"
    );
}

#[test]
fn apply_refuses_files_changed_since_scan_and_writes_nothing() {
    use crate::domain::model::Reference;
    use crate::infra::fs::FileSystem;
    use crate::provider::Resolution;
    use crate::provider::oci::{Digest, Oci};

    let (service, occurrences) = service();
    let find = |path: &str| {
        occurrences
            .iter()
            .find(|o| o.path.as_path() == Path::new(path) && o.reference().is_some())
            .unwrap()
    };
    let (dockerfile, compose) = (find("Dockerfile"), find("compose.yaml"));
    let resolved = |occ: &Occurrence| {
        let Some(Reference::Image(image)) = occ.reference() else {
            panic!("not an image");
        };
        let resolution = Resolution::new::<Oci>(image.clone(), Digest(digest('c')));
        (Reference::Image(image.clone()), Ok(resolution))
    };
    let resolutions = [resolved(dockerfile), resolved(compose)].into();
    let edits = service.plan(&[dockerfile, compose], &resolutions).unwrap();

    let changed = "services:\n  db:\n    image: quay.io/acme/postgres:17\n";
    service
        .fs()
        .write("compose.yaml".as_ref(), changed)
        .unwrap();

    // The Dockerfile comes first and is unchanged, but isn't written either.
    assert!(service.apply(&edits).is_err());
    assert_eq!(service.fs().get("compose.yaml"), changed);
    assert_eq!(service.fs().get("Dockerfile"), DOCKERFILE);
}
