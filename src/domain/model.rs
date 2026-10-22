use std::fmt;
use std::ops::Range;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Ecosystem {
    GithubActions,
    GitlabCi,
    Compose,
    Dockerfile,
    Quadlet,
}

impl Ecosystem {
    pub fn name(self) -> &'static str {
        match self {
            Ecosystem::GithubActions => "github-actions",
            Ecosystem::GitlabCi => "gitlab-ci",
            Ecosystem::Compose => "compose",
            Ecosystem::Dockerfile => "dockerfile",
            Ecosystem::Quadlet => "quadlet",
        }
    }
}

/// Something external a project pulls in.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Reference {
    Git(GitRef),
    Image(ImageRef),
}

/// A git repository at a revision, e.g. `actions/checkout@v4`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GitRef {
    /// Repository URL without `.git`, e.g. `https://github.com/actions/checkout`.
    pub repo_url: String,
    /// Tag, branch or commit SHA.
    pub rev: String,
}

/// A container image, normalized: `node:20` is `docker.io/library/node:20`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ImageRef {
    pub registry: String,
    pub repository: String,
    pub tag: Option<String>,
    pub digest: Option<String>,
}

impl Reference {
    pub fn is_pinned(&self) -> bool {
        match self {
            Reference::Git(g) => crate::domain::refs::is_sha(&g.rev),
            Reference::Image(i) => i.digest.is_some(),
        }
    }
}

impl fmt::Display for Reference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Reference::Git(g) => {
                let repo = g.repo_url.trim_start_matches("https://");
                write!(f, "{repo}@{}", g.rev)
            }
            Reference::Image(i) => {
                write!(f, "{}/{}", i.registry, i.repository)?;
                if let Some(tag) = &i.tag {
                    write!(f, ":{tag}")?;
                }
                if let Some(digest) = &i.digest {
                    write!(f, "@{digest}")?;
                }
                Ok(())
            }
        }
    }
}

/// What an extractor found at a byte range of a file.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub span: Range<usize>,
    pub kind: Kind,
}

impl Found {
    pub fn warning(span: Range<usize>, reason: String) -> Self {
        Found {
            span,
            kind: Kind::Warning(reason),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    /// A reference that can be pinned (or already is). The span is the text that
    /// gets replaced: for git refs only the revision (`v4` in `actions/checkout@v4`),
    /// for images the whole image (`node:20`).
    Ref(Reference),
    /// Something that pulls in external code but can't be pinned, e.g. `FROM node:${V}`.
    Warning(String),
}

/// A `Found` placed in a file.
#[derive(Debug, Clone, PartialEq)]
pub struct Occurrence {
    pub path: PathBuf,
    pub ecosystem: Ecosystem,
    /// 1-based line number, 0 if the warning is about the whole file.
    pub line: usize,
    pub span: Range<usize>,
    /// The text at `span`.
    pub raw: String,
    pub kind: Kind,
}

impl Occurrence {
    pub fn reference(&self) -> Option<&Reference> {
        match &self.kind {
            Kind::Ref(r) => Some(r),
            Kind::Warning(_) => None,
        }
    }
}

/// Replace `span` (whose current text is `expected`) with `replacement`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub path: PathBuf,
    pub span: Range<usize>,
    pub expected: String,
    pub replacement: String,
}
