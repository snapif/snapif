use std::net::{IpAddr, ToSocketAddrs};
use std::time::{Duration, Instant};

use indexmap::IndexMap;
use url::Url;

use crate::backend::{Backend, Evaluated, TYPESAFE_ORIGIN};
use crate::error::{BackendError, Error, PolicyError};
use crate::wire::{self, WireRequest, WireResponse};

const RESPONSE_CAP: usize = 256 * 1024;
const MAX_RETRIES: u32 = 3;

pub struct HttpBackend {
    origin: String,
    api_key: Option<String>,
    typesafe: bool,
    client: reqwest::Client,
}

impl HttpBackend {
    /// Origin `https://api.typesafe.ai`, path `/v1/systemone`, bearer `TYPESAFE_API_KEY`.
    pub fn typesafe(api_key: impl Into<String>) -> Result<Self, Error> {
        let api_key = api_key.into();
        if api_key.is_empty() {
            return Err(Error::Auth("TYPESAFE_API_KEY".to_string()));
        }
        let origin = Url::parse(TYPESAFE_ORIGIN).expect("static origin");
        Self::build(origin, Some(api_key), true)
    }

    /// Caller-supplied origin. The path is always `/v1/systemone`.
    ///
    /// `api_key` is `SNAPIF_API_KEY`. It is optional on loopback and required
    /// otherwise. This constructor never reads `TYPESAFE_API_KEY`.
    pub fn compatible(base_url: Url, api_key: Option<String>) -> Result<Self, Error> {
        validate_origin(&base_url)?;
        let loopback = host_is_loopback(&base_url);
        if !loopback && api_key.as_ref().is_none_or(String::is_empty) {
            return Err(Error::Auth("SNAPIF_API_KEY".to_string()));
        }
        let key = match api_key {
            Some(key) if !key.is_empty() => Some(key),
            _ => None,
        };
        Self::build(base_url, key, false)
    }

    pub(crate) fn endpoint(&self) -> String {
        format!("{}/v1/systemone", self.origin)
    }

    fn build(origin: Url, api_key: Option<String>, typesafe: bool) -> Result<Self, Error> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|err| Error::Backend(err.to_string()))?;
        Ok(Self {
            origin: origin_string(&origin),
            api_key,
            typesafe,
            client,
        })
    }
}

impl Backend for HttpBackend {
    fn id(&self) -> &str {
        if self.typesafe {
            "typesafe"
        } else {
            "compatible"
        }
    }

    async fn evaluate(
        &self,
        req: WireRequest,
        deadline: Instant,
    ) -> Result<Evaluated, BackendError> {
        if Instant::now() >= deadline {
            return Err(BackendError::Timeout);
        }
        let url = self.endpoint();
        let mut retries = 0u32;
        loop {
            if Instant::now() >= deadline {
                return Err(BackendError::Timeout);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            let mut builder = self.client.post(&url).timeout(remaining).json(&req);
            if let Some(key) = &self.api_key {
                builder = builder.bearer_auth(key);
            }
            let response = match builder.send().await {
                Ok(response) => response,
                Err(err) if err.is_timeout() => return Err(BackendError::Timeout),
                Err(err) => return Err(BackendError::Transport(err.to_string())),
            };
            let status = response.status().as_u16();
            if response.status().is_success() {
                let bytes = read_limited(response).await?;
                let wire: WireResponse = wire::decode_response(&bytes)
                    .map_err(|err| BackendError::Transport(err.to_string()))?;
                return Ok(Evaluated {
                    wire,
                    meta: IndexMap::new(),
                    backend_id: self.id().to_string(),
                });
            }
            if response.status().is_redirection() {
                return Err(BackendError::Transport(format!("redirect {status}")));
            }
            if status == 429 || status == 529 {
                if retries >= MAX_RETRIES {
                    return Err(status_error(status, &[]));
                }
                let wait = retry_delay(response.headers(), retries, deadline);
                let Some(wait) = wait else {
                    return Err(status_error(status, &[]));
                };
                tokio::time::sleep(wait).await;
                retries += 1;
                continue;
            }
            let body = read_limited(response).await.unwrap_or_default();
            return Err(status_error(status, &body));
        }
    }
}

fn status_error(status: u16, body: &[u8]) -> BackendError {
    let body = String::from_utf8_lossy(body).into_owned();
    match status {
        401 => BackendError::Auth,
        429 => BackendError::RateLimit,
        529 => BackendError::Overloaded,
        422 | 404 => BackendError::Rejected { status, body },
        _ => BackendError::Transport(format!("HTTP {status}: {body}")),
    }
}

fn retry_delay(
    headers: &reqwest::header::HeaderMap,
    retry_index: u32,
    deadline: Instant,
) -> Option<Duration> {
    let now = Instant::now();
    if now >= deadline {
        return None;
    }
    let remaining = deadline.saturating_duration_since(now);
    let header = headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim);
    let requested = match header {
        Some("") | None => Duration::from_millis(100) * 2u32.pow(retry_index),
        Some(text) => text
            .parse::<u64>()
            .map(Duration::from_secs)
            .unwrap_or_else(|_| Duration::from_millis(100) * 2u32.pow(retry_index)),
    };
    if requested > remaining {
        None
    } else {
        Some(requested)
    }
}

async fn read_limited(mut response: reqwest::Response) -> Result<Vec<u8>, BackendError> {
    let mut buf = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|err| BackendError::Transport(err.to_string()))?
    {
        if buf.len().saturating_add(chunk.len()) > RESPONSE_CAP {
            return Err(BackendError::Transport("response cap".to_string()));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

fn validate_origin(url: &Url) -> Result<(), Error> {
    if !url.username().is_empty() || url.password().is_some() {
        return Err(policy("origin userinfo"));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(policy("origin query"));
    }
    if url.path() != "/" && !url.path().is_empty() {
        return Err(policy("origin path"));
    }
    match url.scheme() {
        "https" => Ok(()),
        "http" if host_is_loopback(url) => Ok(()),
        "http" => Err(policy("http origin must resolve to loopback")),
        _ => Err(policy("origin scheme")),
    }
}

fn host_is_loopback(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    let port = url.port_or_known_default().unwrap_or(80);
    let Ok(addrs) = (host, port).to_socket_addrs() else {
        return false;
    };
    let mut saw = false;
    for addr in addrs {
        saw = true;
        if !is_loopback(addr.ip()) {
            return false;
        }
    }
    saw
}

fn is_loopback(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.is_loopback(),
        IpAddr::V6(ip) => ip.is_loopback(),
    }
}

fn origin_string(url: &Url) -> String {
    let mut origin = url.clone();
    origin.set_path("");
    origin.set_query(None);
    origin.set_fragment(None);
    origin.to_string().trim_end_matches('/').to_string()
}

fn policy(message: &str) -> Error {
    Error::Policy(PolicyError::Invariant(message.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typesafe_endpoint_uses_the_fixed_origin() {
        let backend = HttpBackend::typesafe("secret").expect("key");
        assert_eq!(backend.endpoint(), "https://api.typesafe.ai/v1/systemone");
        assert_eq!(backend.id(), "typesafe");
        assert!(HttpBackend::typesafe("").is_err());
    }

    #[test]
    fn compatible_rejects_non_origin_and_public_http() {
        let key = Some("snapif-key".to_string());
        assert!(HttpBackend::compatible(url("http://10.0.0.1"), key.clone()).is_err());
        assert!(HttpBackend::compatible(url("https://example.com/v1"), key.clone()).is_err());
        assert!(HttpBackend::compatible(url("https://example.com?q=1"), key.clone()).is_err());
        assert!(HttpBackend::compatible(url("https://user:pw@example.com"), key.clone()).is_err());
        assert!(HttpBackend::compatible(url("https://example.com#frag"), key).is_err());
        let loopback = HttpBackend::compatible(url("http://127.0.0.1:9"), None).expect("loopback");
        assert_eq!(loopback.id(), "compatible");
        assert_eq!(loopback.endpoint(), "http://127.0.0.1:9/v1/systemone");
        assert!(HttpBackend::compatible(url("https://example.com"), None).is_err());
    }

    fn url(text: &str) -> Url {
        Url::parse(text).expect("url")
    }
}
