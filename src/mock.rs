//! In-memory file system and canned HTTP responses for tests.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Result, anyhow};

use crate::infra::fs::FileSystem;
use crate::infra::http::{Http, Method, Request, Response};

#[derive(Default)]
pub struct MockFs {
    files: Mutex<BTreeMap<PathBuf, String>>,
}

impl MockFs {
    pub fn new(files: &[(&str, &str)]) -> Self {
        let files = files
            .iter()
            .map(|(p, c)| (PathBuf::from(p), c.to_string()))
            .collect();
        MockFs {
            files: Mutex::new(files),
        }
    }

    pub fn get(&self, path: &str) -> String {
        self.files.lock().unwrap()[Path::new(path)].clone()
    }
}

impl FileSystem for MockFs {
    fn read(&self, path: &Path) -> Result<String> {
        let files = self.files.lock().unwrap();
        files
            .get(path)
            .cloned()
            .ok_or_else(|| anyhow!("{} not found", path.display()))
    }

    fn write(&self, path: &Path, contents: &str) -> Result<()> {
        self.files
            .lock()
            .unwrap()
            .insert(path.to_path_buf(), contents.to_string());
        Ok(())
    }
}

/// A git smart-HTTP ref advertisement, as served by `info/refs`, from
/// `(sha, refname)` lines.
pub fn ref_advertisement(refs: &[(&str, &str)]) -> Vec<u8> {
    let pkt = |s: &str| format!("{:04x}{s}", s.len() + 4);
    let mut body = pkt("# service=git-upload-pack\n") + "0000";
    for (i, (sha, name)) in refs.iter().enumerate() {
        let caps = if i == 0 {
            "\0multi_ack side-band-64k"
        } else {
            ""
        };
        body += &pkt(&format!("{sha} {name}{caps}\n"));
    }
    (body + "0000").into_bytes()
}

/// Answers requests by exact method + URL. If several responses are registered
/// for the same request they are returned in order, the last one repeating
/// (e.g. 401 first, then 200 once a token is sent). Unknown requests are an
/// error, so a test fails loudly when the code calls something unexpected.
#[derive(Default)]
pub struct MockHttp {
    routes: Vec<(Method, String, Response)>,
    calls: Mutex<Vec<Request>>,
}

impl MockHttp {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn on(mut self, method: Method, url: &str, response: Response) -> Self {
        self.routes.push((method, url.to_string(), response));
        self
    }

    /// All requests made so far, in order.
    pub fn calls(&self) -> Vec<Request> {
        self.calls.lock().unwrap().clone()
    }
}

impl Http for MockHttp {
    async fn send(&self, req: Request) -> Result<Response> {
        let mut calls = self.calls.lock().unwrap();
        let same = |r: &Request| r.method == req.method && r.url == req.url;
        let previous = calls.iter().filter(|r| same(r)).count();
        calls.push(req.clone());

        let responses: Vec<&Response> = self
            .routes
            .iter()
            .filter(|(m, url, _)| *m == req.method && *url == req.url)
            .map(|(_, _, resp)| resp)
            .collect();
        let response = responses.get(previous).or(responses.last());
        response
            .map(|r| (*r).clone())
            .ok_or_else(|| anyhow!("no mock for {:?} {}", req.method, req.url))
    }
}
