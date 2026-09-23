use std::net::{IpAddr, ToSocketAddrs};
use std::sync::Mutex;
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
    api_key: Mutex<Option<String>>,
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
    /// otherwise. This constructor never reads `TYPESAFE_API_KEY`. `http` is
    /// limited to loopback.
    pub fn compatible(base_url: Url, api_key: Option<String>) -> Result<Self, Error> {
        Self::open_origin(base_url, api_key, false)
    }

    /// Same origin rules as [`Self::compatible`], plus private-network `http`.
    ///
    /// Every resolved address must be loopback, IPv4 link-local, RFC1918, or
    /// IPv6 unique-local or link-local. One public address rejects the set.
    /// IPv4-mapped IPv6 is rejected. The check runs at construction, so a
    /// later DNS answer can differ. A non-loopback origin still requires
    /// `api_key`.
    pub fn compatible_private(base_url: Url, api_key: Option<String>) -> Result<Self, Error> {
        Self::open_origin(base_url, api_key, true)
    }

    fn open_origin(
        base_url: Url,
        api_key: Option<String>,
        allow_private_http: bool,
    ) -> Result<Self, Error> {
        validate_origin(&base_url, allow_private_http)?;
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
            origin: origin.origin().ascii_serialization(),
            api_key: Mutex::new(api_key),
            typesafe,
            client,
        })
    }

    pub(crate) fn current_key(&self) -> Result<Option<String>, BackendError> {
        self.api_key
            .lock()
            .map(|guard| guard.clone())
            .map_err(|_| BackendError::Transport("api key lock".to_string()))
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

    fn replace_api_key(&self, key: Option<String>) -> Result<Option<String>, Error> {
        if key.as_ref().is_some_and(|value| value.is_empty()) || (self.typesafe && key.is_none()) {
            let name = if self.typesafe {
                "TYPESAFE_API_KEY"
            } else {
                "SNAPIF_API_KEY"
            };
            return Err(Error::Auth(name.to_string()));
        }
        let Ok(mut guard) = self.api_key.lock() else {
            return Err(Error::Backend("api key lock".to_string()));
        };
        let previous = guard.clone();
        *guard = key;
        Ok(previous)
    }

    async fn evaluate(
        &self,
        req: WireRequest,
        deadline: Instant,
    ) -> Result<Evaluated, BackendError> {
        if Instant::now() >= deadline {
            return Err(BackendError::Timeout);
        }
        let key = self.current_key()?;
        let url = self.endpoint();
        let mut retries = 0u32;
        loop {
            if Instant::now() >= deadline {
                return Err(BackendError::Timeout);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            let mut builder = self.client.post(&url).timeout(remaining).json(&req);
            if let Some(key) = &key {
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
                let wire: WireResponse = wire::decode_response(&bytes).map_err(|err| {
                    let snippet = host_line(&String::from_utf8_lossy(&bytes));
                    let body = if snippet.is_empty() {
                        "empty body".to_string()
                    } else {
                        format!("body: {snippet}")
                    };
                    BackendError::Transport(format!("HTTP {status}: {err}; {body}"))
                })?;
                return Ok(Evaluated {
                    wire,
                    meta: IndexMap::new(),
                    backend_id: self.id().to_string(),
                });
            }
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("");
                let target = safe_location(location);
                return Err(BackendError::Transport(if target.is_empty() {
                    format!("redirect {status}")
                } else {
                    format!("redirect {status} to {target}")
                }));
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
            let body = read_limited(response).await?;
            return Err(status_error(status, &body));
        }
    }
}

fn host_line(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    const MAX: usize = 80;
    if flat.len() <= MAX {
        return flat;
    }
    let mut end = MAX;
    while !flat.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &flat[..end])
}

fn safe_location(raw: &str) -> String {
    let Ok(mut url) = url::Url::parse(raw) else {
        return host_line(raw);
    };
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    host_line(url.as_str())
}

fn status_error(status: u16, body: &[u8]) -> BackendError {
    let body = host_line(&String::from_utf8_lossy(body));
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
    // A delay longer than the time still left is not waited out. Sleeping
    // the remainder would burn the gate budget on a retry that cannot finish.
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

fn validate_origin(url: &Url, allow_private_http: bool) -> Result<(), Error> {
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
        "http" => match resolved_ips(url) {
            Some(ips) if ips.iter().all(IpAddr::is_loopback) => Ok(()),
            Some(ips) if allow_private_http && addresses_allowed(&ips) => Ok(()),
            _ if allow_private_http => Err(policy("http origin must resolve to a private address")),
            _ => Err(policy("http origin must resolve to loopback")),
        },
        _ => Err(policy("origin scheme")),
    }
}

fn resolved_ips(url: &Url) -> Option<Vec<IpAddr>> {
    match url.host()? {
        url::Host::Ipv4(ip) => Some(vec![IpAddr::V4(ip)]),
        url::Host::Ipv6(ip) => Some(vec![IpAddr::V6(ip)]),
        url::Host::Domain(host) => {
            let port = url.port_or_known_default().unwrap_or(80);
            let ips: Vec<_> = (host, port)
                .to_socket_addrs()
                .ok()?
                .map(|addr| addr.ip())
                .collect();
            if ips.is_empty() { None } else { Some(ips) }
        }
    }
}

fn addresses_allowed(ips: &[IpAddr]) -> bool {
    !ips.is_empty() && ips.iter().copied().all(ip_is_allowed_private)
}

fn ip_is_allowed_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => {
            v6.to_ipv4_mapped().is_none()
                && (v6.is_loopback() || v6.is_unique_local() || v6.is_unicast_link_local())
        }
    }
}

fn host_is_loopback(url: &Url) -> bool {
    resolved_ips(url).is_some_and(|ips| ips.iter().all(IpAddr::is_loopback))
}

fn policy(message: &str) -> Error {
    Error::Policy(PolicyError::Config(message.to_string()))
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
        let before = backend.endpoint();
        assert!(backend.replace_api_key(Some(String::new())).is_err());
        assert_eq!(
            backend.current_key().expect("key").as_deref(),
            Some("secret")
        );
        backend
            .replace_api_key(Some("next-key".to_string()))
            .expect("swap");
        assert_eq!(
            backend.current_key().expect("key").as_deref(),
            Some("next-key")
        );
        assert_eq!(backend.endpoint(), before);
        assert!(backend.replace_api_key(None).is_err());
        assert_eq!(
            backend.current_key().expect("key").as_deref(),
            Some("next-key")
        );
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
        let first =
            HttpBackend::compatible(url("http://127.0.0.1:9"), Some("loop-key".to_string()))
                .expect("loop");
        let fallback = HttpBackend::typesafe("ts-key").expect("typesafe");
        let cascaded = crate::backends::cascade::Cascaded::new(
            first,
            fallback,
            crate::backends::cascade::CascadeRule::new(0.8),
        );
        assert!(cascaded.replace_api_key(None).is_err());
        assert_eq!(
            cascaded.first.current_key().expect("key").as_deref(),
            Some("loop-key")
        );
        assert_eq!(
            cascaded.fallback.current_key().expect("key").as_deref(),
            Some("ts-key")
        );
        assert!(HttpBackend::compatible(url("https://example.com"), None).is_err());
    }

    #[test]
    fn ipv6_loopback_http_does_not_need_the_private_flag() {
        let backend = HttpBackend::compatible(url("http://[::1]:9"), None).expect("loopback");
        assert_eq!(backend.id(), "compatible");
        assert_eq!(backend.endpoint(), "http://[::1]:9/v1/systemone");
    }

    #[test]
    fn private_http_stays_off_without_the_opt_in() {
        let key = Some("snapif-key".to_string());
        for origin in ["http://192.168.1.50", "http://10.0.0.1", "http://[fd00::1]"] {
            assert!(
                HttpBackend::compatible(url(origin), key.clone()).is_err(),
                "{origin}"
            );
        }
    }

    #[test]
    fn private_http_opt_in_allows_lan_literals() {
        let key = Some("snapif-key".to_string());
        for origin in [
            "http://192.168.1.50",
            "http://10.0.0.1",
            "http://172.16.0.1",
            "http://169.254.1.1",
            "http://[fd00::1]",
            "http://[fe80::1]",
        ] {
            let backend = HttpBackend::compatible_private(url(origin), key.clone())
                .unwrap_or_else(|err| panic!("{origin} should be private http: {err}"));
            assert_eq!(backend.id(), "compatible");
            assert!(
                backend.endpoint().ends_with("/v1/systemone"),
                "{}",
                backend.endpoint()
            );
        }
    }

    #[test]
    fn private_http_opt_in_still_rejects_public_and_non_origins() {
        let key = Some("snapif-key".to_string());
        for origin in [
            "http://8.8.8.8",
            "http://100.64.0.1",
            "http://0.0.0.0",
            "http://[::ffff:192.168.1.1]",
            "http://192.168.1.50/v1",
            "http://user:pw@192.168.1.50",
            "http://192.168.1.50?q=1",
            "http://192.168.1.50#frag",
        ] {
            assert!(
                HttpBackend::compatible_private(url(origin), key.clone()).is_err(),
                "{origin}"
            );
        }
    }

    #[test]
    fn private_http_without_a_key_is_auth() {
        let err = HttpBackend::compatible_private(url("http://192.168.1.50"), None);
        assert!(matches!(err, Err(Error::Auth(_))));
    }

    #[test]
    fn mixed_public_address_is_refused() {
        let private = [
            "192.168.1.50".parse().expect("lan"),
            "10.0.0.1".parse().expect("lan"),
        ];
        assert!(addresses_allowed(&private));
        let mixed = [
            "192.168.1.50".parse().expect("lan"),
            "8.8.8.8".parse().expect("public"),
        ];
        assert!(!addresses_allowed(&mixed));
        assert!(!addresses_allowed(&[]));
    }

    fn url(text: &str) -> Url {
        Url::parse(text).expect("url")
    }
}
