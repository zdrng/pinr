use std::future::Future;

use anyhow::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Head,
}

#[derive(Debug, Clone)]
pub struct Request {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
}

impl Request {
    pub fn get(url: impl Into<String>) -> Self {
        Request {
            method: Method::Get,
            url: url.into(),
            headers: Vec::new(),
        }
    }

    pub fn head(url: impl Into<String>) -> Self {
        Request {
            method: Method::Head,
            url: url.into(),
            headers: Vec::new(),
        }
    }

    pub fn header(mut self, name: &str, value: impl Into<String>) -> Self {
        self.headers.push((name.to_string(), value.into()));
        self
    }
}

#[derive(Debug, Clone, Default)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    /// Case-insensitive header lookup.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Builders for canned responses in tests.
#[cfg(test)]
impl Response {
    pub fn new(status: u16) -> Self {
        Response {
            status,
            ..Default::default()
        }
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    pub fn with_body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = body.into();
        self
    }
}

/// The only way the app talks to the network, so tests can mock it.
pub trait Http: Send + Sync {
    fn send(&self, req: Request) -> impl Future<Output = Result<Response>> + Send;
}

pub struct ReqwestHttp(reqwest::Client);

impl ReqwestHttp {
    pub fn new() -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(concat!("pinr/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(ReqwestHttp(client))
    }
}

impl Http for ReqwestHttp {
    async fn send(&self, req: Request) -> Result<Response> {
        let method = match req.method {
            Method::Get => reqwest::Method::GET,
            Method::Head => reqwest::Method::HEAD,
        };
        let mut builder = self.0.request(method, &req.url);
        for (name, value) in &req.headers {
            builder = builder.header(name, value);
        }
        let resp = builder.send().await?;
        let status = resp.status().as_u16();
        let headers = resp
            .headers()
            .iter()
            .filter_map(|(k, v)| Some((k.to_string(), v.to_str().ok()?.to_string())))
            .collect();
        let body = resp.bytes().await?.to_vec();
        Ok(Response {
            status,
            headers,
            body,
        })
    }
}
