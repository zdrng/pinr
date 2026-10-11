//! One extractor per file format. An extractor only finds references; it doesn't
//! know how they are resolved.

mod compose;
mod dockerfile;
mod github;
mod gitlab;
mod quadlet;
mod yaml;

use crate::config::Config;
use crate::domain::model::{Ecosystem, Found, Kind, Reference};
use crate::domain::refs::parse_image;

pub trait Extractor {
    /// All references and warnings in a file's contents.
    fn extract(&self, src: &str) -> Vec<Found>;
}

pub fn for_ecosystem(ecosystem: Ecosystem, config: &Config) -> Box<dyn Extractor> {
    match ecosystem {
        Ecosystem::GithubActions => Box::new(github::GithubActions),
        Ecosystem::GitlabCi => Box::new(gitlab::GitlabCi {
            host: config.gitlab_host.clone(),
        }),
        Ecosystem::Compose => Box::new(compose::Compose),
        Ecosystem::Dockerfile => Box::new(dockerfile::Dockerfile),
        Ecosystem::Quadlet => Box::new(quadlet::Quadlet),
    }
}

/// The image `text` starting at byte `start`, or a warning if it can't be pinned.
fn image(start: usize, text: &str) -> Found {
    let kind = match parse_image(text) {
        Ok(image) => Kind::Ref(Reference::Image(image)),
        Err(reason) => Kind::Warning(reason),
    };
    Found {
        span: start..start + text.len(),
        kind,
    }
}
