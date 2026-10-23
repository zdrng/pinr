//! Finds the files pinr understands.

use std::path::{Path, PathBuf};

use crate::domain::model::Ecosystem;

/// Walk `root` (respecting .gitignore) and return every file with its ecosystem.
pub fn discover(root: &Path) -> Vec<(PathBuf, Ecosystem)> {
    let mut files: Vec<_> = ignore::WalkBuilder::new(root)
        .hidden(false) // .github/ and .gitlab-ci.yml are hidden
        .filter_entry(|e| e.file_name() != ".git")
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .filter_map(|e| Some((e.path().to_path_buf(), detect(e.path())?)))
        .collect();
    files.sort();
    files
}

pub fn detect(path: &Path) -> Option<Ecosystem> {
    let name = path.file_name()?.to_str()?;
    let is_yaml = name.ends_with(".yml") || name.ends_with(".yaml");
    let stem = name.trim_end_matches(".yml").trim_end_matches(".yaml");
    let in_dir = |dir: &str| path.parent().is_some_and(|p| p.ends_with(dir));

    if is_yaml && (in_dir(".github/workflows") || stem == "action") {
        Some(Ecosystem::GithubActions)
    } else if is_yaml && in_dir(".forgejo/workflows") {
        Some(Ecosystem::ForgejoActions)
    } else if is_yaml && in_dir(".gitea/workflows") {
        Some(Ecosystem::GiteaActions)
    } else if is_yaml && name.contains("gitlab-ci") {
        Some(Ecosystem::GitlabCi)
    } else if is_yaml && (name.starts_with("compose") || name.starts_with("docker-compose")) {
        Some(Ecosystem::Compose)
    } else if is_yaml && in_dir(".circleci") {
        Some(Ecosystem::CircleCi)
    } else if is_yaml && stem == "bitbucket-pipelines" {
        Some(Ecosystem::BitbucketPipelines)
    } else if is_yaml && (stem == ".woodpecker" || in_dir(".woodpecker")) {
        Some(Ecosystem::Woodpecker)
    } else if is_yaml && stem == ".drone" {
        Some(Ecosystem::Drone)
    } else if is_yaml && stem == ".pre-commit-config" {
        Some(Ecosystem::PreCommit)
    } else if name.ends_with(".dockerignore") {
        // `Dockerfile.dockerignore` holds ignore patterns, not instructions.
        None
    } else if ["Dockerfile", "Containerfile"]
        .iter()
        .any(|base| name == *base || name.starts_with(&format!("{base}.")))
        || name.ends_with(".Dockerfile")
        || name.ends_with(".dockerfile")
        || name.ends_with(".Containerfile")
    {
        Some(Ecosystem::Dockerfile)
    } else if [".container", ".image", ".volume"]
        .iter()
        .any(|ext| name.ends_with(ext))
    {
        Some(Ecosystem::Quadlet)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_ecosystems_by_path() {
        let cases = [
            (".github/workflows/ci.yml", Some(Ecosystem::GithubActions)),
            ("actions/setup/action.yaml", Some(Ecosystem::GithubActions)),
            (
                ".forgejo/workflows/ci.yaml",
                Some(Ecosystem::ForgejoActions),
            ),
            (".gitea/workflows/ci.yml", Some(Ecosystem::GiteaActions)),
            (".gitlab-ci.yml", Some(Ecosystem::GitlabCi)),
            ("ci/build.gitlab-ci.yaml", Some(Ecosystem::GitlabCi)),
            (".circleci/config.yml", Some(Ecosystem::CircleCi)),
            (".circleci/docker-compose.yml", Some(Ecosystem::Compose)),
            (
                "bitbucket-pipelines.yml",
                Some(Ecosystem::BitbucketPipelines),
            ),
            (".woodpecker.yaml", Some(Ecosystem::Woodpecker)),
            (".woodpecker/build.yml", Some(Ecosystem::Woodpecker)),
            (".drone.yml", Some(Ecosystem::Drone)),
            (".pre-commit-config.yaml", Some(Ecosystem::PreCommit)),
            (".pre-commit-hooks.yaml", None),
            ("compose.yaml", Some(Ecosystem::Compose)),
            ("deploy/docker-compose.prod.yml", Some(Ecosystem::Compose)),
            ("Dockerfile", Some(Ecosystem::Dockerfile)),
            ("Dockerfile.dev", Some(Ecosystem::Dockerfile)),
            ("api.Dockerfile", Some(Ecosystem::Dockerfile)),
            ("Containerfile", Some(Ecosystem::Dockerfile)),
            ("Dockerfile.dockerignore", None),
            ("api.Dockerfile.dockerignore", None),
            ("systemd/web.container", Some(Ecosystem::Quadlet)),
            ("systemd/base.image", Some(Ecosystem::Quadlet)),
            ("config.yml", None),
            (".github/dependabot.yml", None),
            ("README.md", None),
        ];
        for (path, expected) in cases {
            assert_eq!(detect(Path::new(path)), expected, "{path}");
        }
    }
}
