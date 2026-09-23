use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use indexmap::IndexMap;

use crate::answer::{ChoiceAnswer, NoulAnswer, ScoreAnswer};
use crate::backends::fake::FakeBackend;
use crate::error::{BackendError, DecodeError, Error, PolicyError};
use crate::ids::QuestionId;
use crate::policy::{Fail, Policy};
use crate::question::{ChoiceLabels, Question, ScoreLabels};
use crate::state::State;
use crate::usage::UsageFn;
use crate::verdict::{Decision, UnsureReason, UntypedDecision};
use crate::wire::{
    self, Usage, WireAnswer, WireQuestion, WireRequest, WireResponse, renormalize_probabilities,
};

/// TypeSafe API origin. The path is always `/v1/systemone`.
pub const TYPESAFE_ORIGIN: &str = "https://api.typesafe.ai";

#[derive(Debug)]
pub struct Evaluated {
    pub wire: WireResponse,
    pub meta: IndexMap<String, AnswerMeta>,
    pub backend_id: String,
}

pub use crate::verdict::{AnswerMeta, CascadeHop};

pub trait Backend: Send + Sync {
    fn id(&self) -> &str;
    fn evaluate(
        &self,
        req: WireRequest,
        deadline: Instant,
    ) -> impl Future<Output = Result<Evaluated, BackendError>> + Send;
}

pub enum AnyBackend {
    Fake(FakeBackend),
    #[cfg(feature = "http")]
    Http(crate::backends::http::HttpBackend),
    #[cfg(feature = "http")]
    CascadeHttp(
        Box<
            crate::backends::cascade::Cascaded<
                crate::backends::http::HttpBackend,
                crate::backends::http::HttpBackend,
            >,
        >,
    ),
}

impl Backend for AnyBackend {
    fn id(&self) -> &str {
        match self {
            AnyBackend::Fake(backend) => backend.id(),
            #[cfg(feature = "http")]
            AnyBackend::Http(backend) => backend.id(),
            #[cfg(feature = "http")]
            AnyBackend::CascadeHttp(backend) => backend.id(),
        }
    }

    async fn evaluate(
        &self,
        req: WireRequest,
        deadline: Instant,
    ) -> Result<Evaluated, BackendError> {
        match self {
            AnyBackend::Fake(backend) => backend.evaluate(req, deadline).await,
            #[cfg(feature = "http")]
            AnyBackend::Http(backend) => backend.evaluate(req, deadline).await,
            #[cfg(feature = "http")]
            AnyBackend::CascadeHttp(backend) => backend.evaluate(req, deadline).await,
        }
    }
}

pub(crate) struct GateCache {
    cap: usize,
    ttl: Duration,
    entries: HashMap<u64, (Instant, crate::verdict::Verdict)>,
}

impl GateCache {
    pub(crate) fn new(cap: usize) -> Self {
        Self {
            cap,
            ttl: Duration::from_secs(30),
            entries: HashMap::new(),
        }
    }
}

/// Fields `from_env` already reads. Hosts can fill this instead of exporting variables.
#[derive(Debug, Clone, Default)]
pub struct ClientConfig {
    pub backend: String,
    pub shadow: bool,
    pub model: Option<String>,
    pub timeout_ms: Option<String>,
    pub policy: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub typesafe_key: Option<String>,
    pub allow_private_http: bool,
    pub cascade: Option<String>,
    pub log_path: Option<PathBuf>,
    /// `Some(0)` leaves the cache off. `Some(n)` keeps n identical gate results.
    pub cache_capacity: Option<usize>,
}

pub struct Client<B: Backend> {
    pub(crate) backend: B,
    pub(crate) policy: Option<Policy>,
    pub(crate) on_usage: Option<UsageFn>,
    pub(crate) timeout: Duration,
    pub(crate) model: String,
    shadow_override: Option<bool>,
    fail_override: Option<Fail>,
    pub(crate) log_path: Option<PathBuf>,
    pub(crate) log_lock: Mutex<()>,
    pub(crate) cache: Option<Mutex<GateCache>>,
}

impl<B: Backend> Client<B> {
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            policy: None,
            on_usage: None,
            timeout: Duration::from_millis(2000),
            model: "jev-latest".to_string(),
            shadow_override: None,
            fail_override: None,
            log_path: None,
            log_lock: Mutex::new(()),
            cache: None,
        }
    }

    /// Overrides `policy.fail` for `gate()`. Does not change `ask()`.
    pub fn fail(mut self, fail: Fail) -> Self {
        self.fail_override = Some(fail);
        self
    }

    pub(crate) fn fail_mode(&self) -> Fail {
        if let Some(fail) = self.fail_override {
            return fail;
        }
        self.policy
            .as_ref()
            .map(|policy| policy.fail)
            .unwrap_or(Fail::Closed)
    }

    /// Overrides `policy.shadow` for `gate()`. Does not rewrite the verdict.
    pub fn shadow(mut self, on: bool) -> Self {
        self.shadow_override = Some(on);
        self
    }

    pub(crate) fn shadow_on(&self, policy_shadow: bool) -> bool {
        self.shadow_override.unwrap_or(policy_shadow)
    }

    pub fn policy(mut self, policy: Policy) -> Self {
        self.policy = Some(policy);
        self
    }

    pub fn on_usage(mut self, on_usage: UsageFn) -> Self {
        self.on_usage = Some(on_usage);
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Sets the wire model id. Empty or whitespace is [`Error::Policy`].
    pub fn model(mut self, id: impl AsRef<str>) -> Result<Self, Error> {
        let id = id.as_ref().trim();
        if id.is_empty() {
            return Err(Error::Policy(PolicyError::Config(
                "SNAPIF_MODEL must not be blank".to_string(),
            )));
        }
        self.model = id.to_string();
        Ok(self)
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// Battery id of the loaded policy, such as `screen` or `tool-gate`.
    pub fn battery_id(&self) -> Option<&str> {
        self.policy.as_ref().map(|policy| policy.battery.0.as_str())
    }

    pub(crate) fn cache_get(&self, key: u64) -> Option<crate::verdict::Verdict> {
        let cache = self.cache.as_ref()?;
        let mut guard = cache.lock().ok()?;
        let expired = guard
            .entries
            .get(&key)
            .is_some_and(|(stored, _)| stored.elapsed() > guard.ttl);
        if expired {
            guard.entries.remove(&key);
            return None;
        }
        guard.entries.get(&key).map(|(_, verdict)| verdict.clone())
    }

    pub(crate) fn cache_put(&self, key: u64, verdict: crate::verdict::Verdict) {
        let Some(cache) = &self.cache else {
            return;
        };
        let Ok(mut guard) = cache.lock() else {
            return;
        };
        if guard.entries.len() >= guard.cap
            && !guard.entries.contains_key(&key)
            && let Some(old) = guard.entries.keys().next().copied()
        {
            guard.entries.remove(&old);
        }
        guard.entries.insert(key, (Instant::now(), verdict));
    }

    /// Observation. Not an execute path.
    ///
    /// Known versus Unsure uses `choice.escalate_below` and `choice.signal`.
    /// Per-action auto thresholds are not consulted, and `prepared` is not injected.
    /// A trimmed untrusted state is reported on [`AskOut`] and does not change the decision.
    pub async fn ask(&self, state: State, questions: Vec<Question>) -> Result<AskOut, Error> {
        let policy = self
            .policy
            .as_ref()
            .ok_or_else(|| Error::Policy(PolicyError::Invariant("policy".to_string())))?;
        policy.ensure_checked()?;
        let mut request = WireRequest {
            model: self.model.clone(),
            state: state.to_wire(None),
            questions: questions.iter().map(wire_question).collect(),
        };
        let encoded = wire::encode(&request)?;
        if encoded.truncated_untrusted {
            request = wire::decode_request(&encoded.body)?;
        }
        let deadline = Instant::now() + self.timeout;
        let evaluated = self
            .backend
            .evaluate(request.clone(), deadline)
            .await
            .map_err(|err| map_backend(err, self.timeout))?;
        if let Some(on_usage) = &self.on_usage {
            on_usage(evaluated.wire.usage);
        }
        wire::check_response(&request.questions, &evaluated.wire)?;
        let Evaluated {
            wire,
            mut meta,
            backend_id,
        } = evaluated;
        let mut decisions = IndexMap::new();
        let mut scores = IndexMap::new();
        for (id, question) in &request.questions {
            let Some(answer) = wire.answers.get(id) else {
                continue;
            };
            record_prob_sum(&mut meta, id, answer);
            scores.insert(id.clone(), answer_score(answer));
            let key = QuestionId::new(id);
            let decision = untyped(policy, question, answer).map_err(Error::Decode)?;
            decisions.insert(key, decision);
        }
        Ok(AskOut {
            decisions,
            scores,
            usage: wire.usage,
            backend_id,
            meta,
            truncated_untrusted: encoded.truncated_untrusted,
        })
    }
}

#[derive(Debug, Default)]
struct BackendEnv {
    #[cfg(feature = "http")]
    typesafe_key: Option<String>,
    #[cfg(feature = "http")]
    snapif_key: Option<String>,
    #[cfg(feature = "http")]
    base_url: Option<String>,
    #[cfg(feature = "http")]
    allow_private_http: bool,
    #[cfg_attr(not(feature = "http"), allow(dead_code))]
    cascade: Option<String>,
    model: Option<String>,
    timeout_ms: Option<String>,
    policy: Option<String>,
}

impl Client<AnyBackend> {
    /// `SNAPIF_BACKEND` is required (`fake`, `typesafe`, or `compatible`).
    ///
    /// Unset or non-Unicode is [`Error::Policy`], not a silent fake.
    /// `SNAPIF_SHADOW` of `1` or `true` sets the shadow override.
    /// `SNAPIF_MODEL` overrides the default `jev-latest`. Empty or whitespace
    /// is [`Error::Policy`]. `SNAPIF_TIMEOUT_MS`
    /// overrides the 2000 ms gate budget. `SNAPIF_POLICY` is a shipped id or
    /// a `.toml` path; unset uses `tool-gate`.
    /// `typesafe` reads `TYPESAFE_API_KEY` and fails with [`Error::Auth`] when
    /// that key is missing. `compatible` reads `SNAPIF_BASE_URL` (origin only)
    /// and `SNAPIF_API_KEY` (optional on loopback). `SNAPIF_ALLOW_PRIVATE_HTTP`
    /// of `1` or `true` also allows `http` when every resolved address is
    /// loopback, link-local, RFC1918, or IPv6 unique-local. Unset keeps `http`
    /// on loopback only. The check is at construction; a later DNS answer can
    /// differ. The same flag applies to `SNAPIF_CASCADE_BASE_URL`. A
    /// non-loopback origin still requires `SNAPIF_API_KEY`. When
    /// `SNAPIF_CASCADE_BASE_URL` is set and the backend is not `fake`, the
    /// first hop is that origin with `SNAPIF_API_KEY` and the fallback is
    /// `typesafe` or `compatible`. Without the `http` feature, `typesafe`,
    /// `compatible`, and a cascade URL stay [`Error::Policy`].
    pub fn from_env() -> Result<Self, Error> {
        let cache_capacity = match std::env::var("SNAPIF_CACHE").ok().as_deref() {
            None => None,
            Some(raw) => {
                let raw = raw.trim();
                if raw.is_empty() {
                    None
                } else {
                    Some(raw.parse::<usize>().map_err(|_| {
                        Error::Policy(PolicyError::Config(format!(
                            "SNAPIF_CACHE must be an integer, got {raw}"
                        )))
                    })?)
                }
            }
        };
        let config = ClientConfig {
            backend: std::env::var("SNAPIF_BACKEND").unwrap_or_default(),
            shadow: matches!(
                std::env::var("SNAPIF_SHADOW").ok().as_deref(),
                Some("1" | "true" | "TRUE" | "True")
            ),
            model: std::env::var("SNAPIF_MODEL").ok(),
            timeout_ms: std::env::var("SNAPIF_TIMEOUT_MS").ok(),
            policy: std::env::var("SNAPIF_POLICY").ok(),
            #[cfg(feature = "http")]
            base_url: std::env::var("SNAPIF_BASE_URL").ok(),
            #[cfg(not(feature = "http"))]
            base_url: None,
            #[cfg(feature = "http")]
            api_key: std::env::var("SNAPIF_API_KEY").ok(),
            #[cfg(not(feature = "http"))]
            api_key: None,
            #[cfg(feature = "http")]
            typesafe_key: std::env::var("TYPESAFE_API_KEY").ok(),
            #[cfg(not(feature = "http"))]
            typesafe_key: None,
            #[cfg(feature = "http")]
            allow_private_http: matches!(
                std::env::var("SNAPIF_ALLOW_PRIVATE_HTTP").ok().as_deref(),
                Some("1" | "true" | "TRUE" | "True")
            ),
            #[cfg(not(feature = "http"))]
            allow_private_http: false,
            cascade: std::env::var("SNAPIF_CASCADE_BASE_URL").ok(),
            log_path: std::env::var("SNAPIF_LOG")
                .ok()
                .map(|raw| raw.trim().to_string())
                .filter(|raw| !raw.is_empty())
                .map(PathBuf::from),
            cache_capacity,
        };
        Self::from_config(&config)
    }

    /// Same client as [`Self::from_env`] for the same values. Does not read the process environment.
    pub fn from_config(config: &ClientConfig) -> Result<Self, Error> {
        let env = BackendEnv {
            #[cfg(feature = "http")]
            typesafe_key: config.typesafe_key.clone(),
            #[cfg(feature = "http")]
            snapif_key: config.api_key.clone(),
            #[cfg(feature = "http")]
            base_url: config.base_url.clone(),
            #[cfg(feature = "http")]
            allow_private_http: config.allow_private_http,
            cascade: config.cascade.clone(),
            model: config.model.clone(),
            timeout_ms: config.timeout_ms.clone(),
            policy: config.policy.clone(),
        };
        let name = match config.backend.trim() {
            "" => None,
            other => Some(other),
        };
        let mut client = Self::from_parts(name, config.shadow, &env)?;
        client.log_path = config.log_path.clone();
        if let Some(cap) = config.cache_capacity.filter(|cap| *cap > 0) {
            client.cache = Some(Mutex::new(GateCache::new(cap)));
        }
        Ok(client)
    }

    #[cfg_attr(not(feature = "http"), allow(unused_variables))]
    fn from_parts(name: Option<&str>, shadow: bool, env: &BackendEnv) -> Result<Self, Error> {
        let name = match name.map(str::trim) {
            None | Some("") => None,
            Some(text) => Some(text),
        };
        let policy = env_policy(env)?;
        let client = match name {
            None => {
                return Err(Error::Policy(PolicyError::BackendName(
                    "SNAPIF_BACKEND must be fake, typesafe, or compatible".to_string(),
                )));
            }
            Some("fake") => Self::new(AnyBackend::Fake(FakeBackend::new())).policy(policy),
            #[cfg(feature = "http")]
            Some(name @ ("typesafe" | "compatible")) => http_client(name, env, policy)?,
            #[cfg(not(feature = "http"))]
            Some(name @ ("typesafe" | "compatible")) => {
                return Err(Error::Policy(PolicyError::BackendName(format!(
                    "SNAPIF_BACKEND {name} needs the http feature"
                ))));
            }
            Some(other) => {
                return Err(Error::Policy(PolicyError::BackendName(format!(
                    "unknown SNAPIF_BACKEND {other}; expected fake, typesafe, or compatible"
                ))));
            }
        };
        let client = apply_runtime(client, env)?;
        Ok(if shadow { client.shadow(true) } else { client })
    }
}

fn env_policy(env: &BackendEnv) -> Result<Policy, Error> {
    match nonempty(env.policy.as_deref()) {
        Some(spec) => Policy::load(spec),
        None => Ok(Policy::shipped("tool-gate")?),
    }
}

fn apply_runtime(
    mut client: Client<AnyBackend>,
    env: &BackendEnv,
) -> Result<Client<AnyBackend>, Error> {
    if let Some(raw) = nonempty(env.timeout_ms.as_deref()) {
        let ms: u64 = raw.parse().map_err(|_| {
            Error::Policy(PolicyError::Config(format!(
                "SNAPIF_TIMEOUT_MS must be an integer, got {raw}"
            )))
        })?;
        client = client.timeout(Duration::from_millis(ms));
    }
    if let Some(raw) = env.model.as_deref() {
        let model = raw.trim();
        if model.is_empty() {
            return Err(Error::Policy(PolicyError::Config(
                "SNAPIF_MODEL must not be blank".to_string(),
            )));
        }
        client.model = model.to_string();
    }
    Ok(client)
}

fn nonempty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|text| !text.is_empty())
}

#[cfg(feature = "http")]
fn http_client(name: &str, env: &BackendEnv, policy: Policy) -> Result<Client<AnyBackend>, Error> {
    let backend = if let Some(raw) = nonempty(env.cascade.as_deref()) {
        let first = compatible_backend(
            raw,
            env.snapif_key.clone(),
            "SNAPIF_CASCADE_BASE_URL",
            env.allow_private_http,
        )?;
        let fallback = match name {
            "typesafe" => crate::backends::http::HttpBackend::typesafe(
                env.typesafe_key.clone().unwrap_or_default(),
            )?,
            "compatible" => {
                let Some(base) = env.base_url.as_deref().filter(|value| !value.is_empty()) else {
                    return Err(Error::Policy(PolicyError::Config(
                        "SNAPIF_BASE_URL is required".to_string(),
                    )));
                };
                compatible_backend(
                    base,
                    env.snapif_key.clone(),
                    "SNAPIF_BASE_URL",
                    env.allow_private_http,
                )?
            }
            _ => return Err(Error::Policy(PolicyError::Invariant(name.to_string()))),
        };
        let rule = crate::backends::cascade::CascadeRule::new(policy.cascade_min);
        AnyBackend::CascadeHttp(Box::new(crate::backends::cascade::Cascaded::new(
            first, fallback, rule,
        )))
    } else {
        match name {
            "typesafe" => AnyBackend::Http(crate::backends::http::HttpBackend::typesafe(
                env.typesafe_key.clone().unwrap_or_default(),
            )?),
            "compatible" => {
                let Some(base) = env.base_url.as_deref().filter(|value| !value.is_empty()) else {
                    return Err(Error::Policy(PolicyError::Config(
                        "SNAPIF_BASE_URL is required".to_string(),
                    )));
                };
                AnyBackend::Http(compatible_backend(
                    base,
                    env.snapif_key.clone(),
                    "SNAPIF_BASE_URL",
                    env.allow_private_http,
                )?)
            }
            _ => return Err(Error::Policy(PolicyError::Invariant(name.to_string()))),
        }
    };
    Ok(Client::new(backend).policy(policy))
}

#[cfg(feature = "http")]
fn compatible_backend(
    raw: &str,
    key: Option<String>,
    invariant: &str,
    allow_private_http: bool,
) -> Result<crate::backends::http::HttpBackend, Error> {
    let url = url::Url::parse(raw)
        .map_err(|_| Error::Policy(PolicyError::Config(format!("{invariant} must be a URL"))))?;
    if allow_private_http {
        crate::backends::http::HttpBackend::compatible_private(url, key)
    } else {
        crate::backends::http::HttpBackend::compatible(url, key)
    }
}

#[derive(Debug)]
#[non_exhaustive]
pub struct AskOut {
    pub decisions: IndexMap<QuestionId, UntypedDecision>,
    /// Choice confidence, score value, or noul probability, keyed by question id.
    pub scores: IndexMap<String, f64>,
    pub usage: Usage,
    pub backend_id: String,
    pub meta: IndexMap<String, AnswerMeta>,
    /// Encode replaced `state.untrusted` so the body fit the wire cap.
    pub truncated_untrusted: bool,
}

impl AskOut {
    pub fn choice<T: ChoiceLabels>(&self, id: &QuestionId) -> Result<Decision<T>, DecodeError> {
        match self.decisions.get(id) {
            None => Err(DecodeError::MissingAnswer { key: id.clone() }),
            Some(UntypedDecision::Choice(Decision::Known(label))) => match T::from_label(label) {
                Some(value) => Ok(Decision::Known(value)),
                None => Err(DecodeError::UnknownLabel {
                    key: id.clone(),
                    label: label.clone(),
                }),
            },
            Some(UntypedDecision::Choice(Decision::Unsure { reason, guess })) => {
                Ok(Decision::Unsure {
                    reason: reason.clone(),
                    guess: guess.as_deref().and_then(T::from_label),
                })
            }
            Some(_) => Err(DecodeError::TypeMismatch { key: id.clone() }),
        }
    }

    pub fn score<T: ScoreLabels>(&self, id: &QuestionId) -> Result<Decision<T>, DecodeError> {
        match self.decisions.get(id) {
            None => Err(DecodeError::MissingAnswer { key: id.clone() }),
            Some(UntypedDecision::Score(Decision::Known(score))) => {
                score_label::<T>(*score, id.clone())
            }
            Some(UntypedDecision::Score(Decision::Unsure { reason, guess })) => {
                Ok(Decision::Unsure {
                    reason: reason.clone(),
                    guess: guess.and_then(|score| score_index(score).and_then(T::from_index)),
                })
            }
            Some(_) => Err(DecodeError::TypeMismatch { key: id.clone() }),
        }
    }

    pub fn noul(&self, id: &QuestionId) -> Result<Decision<bool>, DecodeError> {
        match self.decisions.get(id) {
            None => Err(DecodeError::MissingAnswer { key: id.clone() }),
            Some(UntypedDecision::Noul(decision)) => Ok(decision.clone()),
            Some(_) => Err(DecodeError::TypeMismatch { key: id.clone() }),
        }
    }
}

fn score_label<T: ScoreLabels>(score: f64, id: QuestionId) -> Result<Decision<T>, DecodeError> {
    match score_index(score).and_then(T::from_index) {
        Some(value) => Ok(Decision::Known(value)),
        None => Err(DecodeError::OutOfRange { key: id }),
    }
}

fn score_index(score: f64) -> Option<usize> {
    if score.is_finite() && score >= 0.0 {
        Some(score.round() as usize)
    } else {
        None
    }
}

fn map_backend(err: BackendError, timeout: Duration) -> Error {
    match err {
        BackendError::Timeout => Error::Timeout(timeout),
        BackendError::RateLimit => Error::RateLimit,
        BackendError::Overloaded => Error::Overloaded,
        BackendError::Auth => Error::Auth("authentication failed (HTTP 401)".to_string()),
        BackendError::Rejected { status, body } => Error::Rejected { status, body },
        other => Error::Backend(other.to_string()),
    }
}

pub(crate) fn wire_question(question: &Question) -> (String, WireQuestion) {
    match question {
        Question::Choice(choice) => (
            choice.id.to_string(),
            WireQuestion::Choice {
                instructions: choice.instructions.clone(),
                criteria: choice.criteria.clone(),
            },
        ),
        Question::Score(score) => (
            score.id.to_string(),
            WireQuestion::Score {
                instructions: score.instructions.clone(),
                criteria: score.criteria.clone(),
            },
        ),
        Question::Noul(noul) => (
            noul.id.to_string(),
            WireQuestion::Noul {
                instructions: noul.instructions.clone(),
                criteria: noul.criteria.clone(),
            },
        ),
    }
}

fn answer_score(answer: &WireAnswer) -> f64 {
    match answer {
        WireAnswer::Choice { confidence, .. } => *confidence,
        WireAnswer::Score { score, .. } => *score,
        WireAnswer::Noul { noul } => *noul,
    }
}

pub(crate) fn record_prob_sum(
    meta: &mut IndexMap<String, AnswerMeta>,
    id: &str,
    answer: &WireAnswer,
) {
    let probabilities = match answer {
        WireAnswer::Choice { probabilities, .. } | WireAnswer::Score { probabilities, .. } => {
            probabilities
        }
        WireAnswer::Noul { .. } => return,
    };
    let (_, original_sum) = renormalize_probabilities(probabilities);
    if (original_sum - 1.0).abs() > 1e-6 {
        meta.entry(id.to_string()).or_default().original_prob_sum = Some(original_sum);
    }
}

fn untyped(
    policy: &Policy,
    question: &WireQuestion,
    answer: &WireAnswer,
) -> Result<UntypedDecision, DecodeError> {
    match (question, answer) {
        (
            WireQuestion::Choice { .. },
            WireAnswer::Choice {
                choice,
                probabilities,
                confidence,
            },
        ) => {
            let decoded = ChoiceAnswer {
                label: choice.clone(),
                confidence: *confidence,
                probabilities: probabilities.clone(),
            };
            let signal = decoded.signal(policy.choice.signal);
            let floor = policy.choice.escalate_below;
            let decision = if signal < floor {
                Decision::Unsure {
                    reason: UnsureReason::BelowFloor {
                        confidence: signal,
                        floor,
                    },
                    guess: Some(decoded.label),
                }
            } else {
                Decision::Known(decoded.label)
            };
            Ok(UntypedDecision::Choice(decision))
        }
        (
            WireQuestion::Score { .. },
            WireAnswer::Score {
                score,
                probabilities,
                confidence,
                ..
            },
        ) => {
            let decoded = ScoreAnswer {
                score: *score,
                confidence: *confidence,
                probabilities: probabilities.clone(),
            };
            let signal = decoded.signal(policy.choice.signal);
            let floor = policy.choice.escalate_below;
            let decision = if signal < floor {
                Decision::Unsure {
                    reason: UnsureReason::BelowFloor {
                        confidence: signal,
                        floor,
                    },
                    guess: Some(decoded.score),
                }
            } else {
                Decision::Known(decoded.score)
            };
            Ok(UntypedDecision::Score(decision))
        }
        (WireQuestion::Noul { .. }, WireAnswer::Noul { noul }) => {
            let decision = NoulAnswer { p: *noul }.decide(&policy.noul, None);
            Ok(UntypedDecision::Noul(decision))
        }
        _ => Err(DecodeError::TypeMismatch {
            key: QuestionId::new("answer"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::{AnyBackend, Backend, Client, ClientConfig};
    use crate::error::{Error, PolicyError};

    #[test]
    fn from_name_selects_without_env() {
        let env = super::BackendEnv::default();
        let config = ClientConfig {
            backend: "fake".to_string(),
            timeout_ms: Some("nope".to_string()),
            ..ClientConfig::default()
        };
        let bad = match Client::<AnyBackend>::from_config(&config) {
            Err(err) => err,
            Ok(_) => panic!("timeout"),
        };
        assert!(bad.to_string().contains("SNAPIF_TIMEOUT_MS"), "{bad}");
        let ok = Client::<AnyBackend>::from_config(&ClientConfig {
            backend: "fake".to_string(),
            ..ClientConfig::default()
        })
        .expect("fake");
        assert_eq!(ok.backend().id(), "fake");
        let Err(unset) = Client::<AnyBackend>::from_parts(None, false, &env) else {
            panic!("unset must be policy");
        };
        assert!(matches!(
            unset,
            Error::Policy(PolicyError::BackendName(ref message))
                if message.contains("SNAPIF_BACKEND")
                    && message.contains("fake")
                    && message.contains("typesafe")
                    && message.contains("compatible")
        ));
        let spaced =
            Client::<AnyBackend>::from_parts(Some("  fake  "), false, &env).expect("trimmed fake");
        assert_eq!(spaced.backend().id(), "fake");
        let shown = unset.to_string();
        assert!(!shown.contains("threshold invariant"), "{shown}");
        let Err(unknown_name) = Client::<AnyBackend>::from_parts(Some("laya"), false, &env) else {
            panic!("unknown backend");
        };
        let unknown_text = unknown_name.to_string();
        assert!(
            unknown_text.contains("unknown SNAPIF_BACKEND laya"),
            "{unknown_text}"
        );
        assert!(unknown_text.contains("fake"), "{unknown_text}");
        assert!(
            !unknown_text.contains("threshold invariant"),
            "{unknown_text}"
        );

        let client = Client::<AnyBackend>::from_parts(Some("fake"), false, &env).expect("fake");
        assert_eq!(client.backend().id(), "fake");
        let shadowed = Client::<AnyBackend>::from_parts(Some("fake"), true, &env).expect("shadow");
        assert_eq!(shadowed.shadow_override, Some(true));

        #[cfg(not(feature = "http"))]
        {
            let Err(unknown) = Client::<AnyBackend>::from_parts(Some("typesafe"), false, &env)
            else {
                panic!("typesafe must be policy");
            };
            assert!(matches!(
                unknown,
                Error::Policy(PolicyError::BackendName(ref message))
                    if message.contains("typesafe") && message.contains("http feature")
            ));
            assert!(!unknown.to_string().contains("threshold invariant"));
            let cascade_env = super::BackendEnv {
                cascade: Some("http://127.0.0.1:9".to_string()),
                ..super::BackendEnv::default()
            };
            let err = Client::<AnyBackend>::from_parts(Some("typesafe"), false, &cascade_env);
            assert!(matches!(err, Err(Error::Policy(_))));
        }
        #[cfg(feature = "http")]
        {
            let Err(missing_key) = Client::<AnyBackend>::from_parts(Some("typesafe"), false, &env)
            else {
                panic!("typesafe without a key must be auth");
            };
            assert!(matches!(missing_key, Error::Auth(_)));
            let selected = Client::<AnyBackend>::from_parts(
                Some("typesafe"),
                false,
                &super::BackendEnv {
                    typesafe_key: Some("secret".to_string()),
                    ..super::BackendEnv::default()
                },
            )
            .expect("typesafe");
            assert_eq!(selected.backend().id(), "typesafe");
            let cascaded = Client::<AnyBackend>::from_parts(
                Some("typesafe"),
                false,
                &super::BackendEnv {
                    typesafe_key: Some("secret".to_string()),
                    cascade: Some("http://127.0.0.1:9".to_string()),
                    ..super::BackendEnv::default()
                },
            )
            .expect("cascade");
            assert_eq!(cascaded.backend().id(), "cascade");
        }

        let tuned = Client::<AnyBackend>::from_parts(
            Some("fake"),
            false,
            &super::BackendEnv {
                model: Some("custom-model".to_string()),
                timeout_ms: Some("1500".to_string()),
                ..super::BackendEnv::default()
            },
        )
        .expect("runtime");
        assert_eq!(tuned.model, "custom-model");
        assert_eq!(tuned.timeout, std::time::Duration::from_millis(1500));

        let bad_timeout = Client::<AnyBackend>::from_parts(
            Some("fake"),
            false,
            &super::BackendEnv {
                timeout_ms: Some("nope".to_string()),
                ..super::BackendEnv::default()
            },
        );
        assert!(matches!(bad_timeout, Err(Error::Policy(_))));
        let bad_policy = Client::<AnyBackend>::from_parts(
            Some("fake"),
            false,
            &super::BackendEnv {
                policy: Some("missing-policy".to_string()),
                ..super::BackendEnv::default()
            },
        );
        assert!(matches!(bad_policy, Err(Error::Policy(_))));
    }

    #[test]
    fn model_env_rejects_blank_and_keeps_the_default() {
        let default =
            Client::<AnyBackend>::from_parts(Some("fake"), false, &super::BackendEnv::default())
                .expect("fake");
        assert_eq!(default.model, "jev-latest");

        for blank in ["", "   ", "\t"] {
            let err = Client::<AnyBackend>::from_parts(
                Some("fake"),
                false,
                &super::BackendEnv {
                    model: Some(blank.to_string()),
                    ..super::BackendEnv::default()
                },
            );
            let Err(err) = err else {
                panic!("blank model must fail");
            };
            assert_eq!(
                err.to_string(),
                "policy: SNAPIF_MODEL must not be blank",
                "{blank:?} {err}"
            );
        }

        let trimmed = Client::<AnyBackend>::from_parts(
            Some("fake"),
            false,
            &super::BackendEnv {
                model: Some("  local-model  ".to_string()),
                ..super::BackendEnv::default()
            },
        )
        .expect("trimmed");
        assert_eq!(trimmed.model, "local-model");
    }

    #[cfg(feature = "http")]
    #[test]
    fn private_http_flag_is_off_unless_set() {
        let off = Client::<AnyBackend>::from_parts(
            Some("compatible"),
            false,
            &super::BackendEnv {
                base_url: Some("http://10.0.0.1".to_string()),
                snapif_key: Some("snapif-key".to_string()),
                ..super::BackendEnv::default()
            },
        );
        assert!(matches!(off, Err(Error::Policy(_))));

        let on = Client::<AnyBackend>::from_parts(
            Some("compatible"),
            false,
            &super::BackendEnv {
                base_url: Some("http://10.0.0.1".to_string()),
                snapif_key: Some("snapif-key".to_string()),
                allow_private_http: true,
                ..super::BackendEnv::default()
            },
        );
        let Ok(on) = on else {
            panic!("private http");
        };
        assert_eq!(on.backend().id(), "compatible");
    }

    #[cfg(feature = "http")]
    #[test]
    fn private_http_flag_covers_the_cascade_origin() {
        let denied = Client::<AnyBackend>::from_parts(
            Some("compatible"),
            false,
            &super::BackendEnv {
                base_url: Some("http://10.0.0.1".into()),
                cascade: Some("http://192.168.1.50".into()),
                snapif_key: Some("snapif-key".into()),
                allow_private_http: false,
                ..super::BackendEnv::default()
            },
        );
        assert!(matches!(denied, Err(Error::Policy(_))));

        let allowed = Client::<AnyBackend>::from_parts(
            Some("compatible"),
            false,
            &super::BackendEnv {
                base_url: Some("http://10.0.0.1".into()),
                cascade: Some("http://192.168.1.50".into()),
                snapif_key: Some("snapif-key".into()),
                allow_private_http: true,
                ..super::BackendEnv::default()
            },
        );
        let Ok(allowed) = allowed else {
            panic!("private http cascade");
        };
        assert_eq!(allowed.backend().id(), "cascade");
    }

    #[test]
    fn auth_error_reaches_the_client_text() {
        let err = super::map_backend(
            crate::error::BackendError::Auth,
            std::time::Duration::from_secs(1),
        );
        assert_eq!(err.to_string(), "auth: authentication failed (HTTP 401)");
    }
}
