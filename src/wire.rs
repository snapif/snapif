use crate::error::{DecodeError, WireError};
use crate::ids::QuestionId;
use indexmap::IndexMap;
use serde::Serialize;
use serde_json::Value;

pub const ENCODE_CAP: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EncodedRequest {
    pub body: Vec<u8>,
    pub truncated_untrusted: bool,
}

fn default_model() -> String {
    "jev-latest".to_string()
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WireRequest {
    #[serde(default = "default_model")]
    pub model: String,
    pub state: Value,
    pub questions: IndexMap<String, WireQuestion>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum WireQuestion {
    Choice {
        instructions: Value,
        criteria: IndexMap<String, Value>,
    },
    Score {
        instructions: Value,
        criteria: Vec<Value>,
    },
    Noul {
        instructions: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        criteria: Option<NoulCriteria>,
    },
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NoulCriteria {
    #[serde(rename = "true", skip_serializing_if = "Option::is_none")]
    pub r#true: Option<Value>,
    #[serde(rename = "false", skip_serializing_if = "Option::is_none")]
    pub r#false: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WireResponse {
    pub model: String,
    pub answers: IndexMap<String, WireAnswer>,
    pub usage: Usage,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum WireAnswer {
    Choice {
        choice: String,
        probabilities: IndexMap<String, f64>,
        confidence: f64,
    },
    Score {
        score: f64,
        legend: IndexMap<String, String>,
        probabilities: IndexMap<String, f64>,
        confidence: f64,
    },
    /// Extra keys, including `confidence`, are ignored.
    Noul { noul: f64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

fn json_error(err: impl ToString) -> WireError {
    WireError::Json(err.to_string())
}

fn to_vec(value: &impl Serialize) -> Result<Vec<u8>, WireError> {
    serde_json::to_vec(value).map_err(json_error)
}

fn known_type(value: &Value) -> Result<(), WireError> {
    let name = match value.get("type").and_then(Value::as_str) {
        None => "missing".to_string(),
        Some("") => "empty".to_string(),
        Some(name) => name.to_string(),
    };
    if matches!(name.as_str(), "choice" | "score" | "noul") {
        Ok(())
    } else {
        Err(WireError::UnknownType(name))
    }
}

fn validate(request: &WireRequest) -> Result<(), WireError> {
    for question in request.questions.values() {
        match question {
            WireQuestion::Choice { criteria, .. } => {
                let len = criteria.len();
                if len == 0 {
                    return Err(WireError::EmptyChoice);
                }
                if len > 255 {
                    return Err(WireError::ChoiceTooWide);
                }
            }
            WireQuestion::Score { criteria, .. } => {
                let len = criteria.len();
                if !(2..=10).contains(&len) {
                    return Err(WireError::ScoreLen(len));
                }
            }
            WireQuestion::Noul { .. } => {}
        }
    }
    Ok(())
}

pub fn decode_request(bytes: &[u8]) -> Result<WireRequest, WireError> {
    let value: Value = serde_json::from_slice(bytes).map_err(json_error)?;
    if let Some(questions) = value.get("questions").and_then(Value::as_object) {
        for question in questions.values() {
            known_type(question)?;
        }
    }
    let request: WireRequest = serde_json::from_value(value).map_err(json_error)?;
    validate(&request)?;
    Ok(request)
}

pub fn decode_response(bytes: &[u8]) -> Result<WireResponse, WireError> {
    let value: Value = serde_json::from_slice(bytes).map_err(json_error)?;
    if let Some(answers) = value.get("answers").and_then(Value::as_object) {
        for answer in answers.values() {
            known_type(answer)?;
        }
    }
    serde_json::from_value(value).map_err(json_error)
}

pub fn encode(request: &WireRequest) -> Result<EncodedRequest, WireError> {
    validate(request)?;
    let body = to_vec(request)?;
    if body.len() <= ENCODE_CAP {
        return Ok(EncodedRequest {
            body,
            truncated_untrusted: false,
        });
    }
    let mut trimmed = request.clone();
    let Value::Object(map) = &mut trimmed.state else {
        return Err(WireError::BodyCap(body.len()));
    };
    map.insert(
        "untrusted".to_string(),
        serde_json::json!({"truncated": true}),
    );
    let body = to_vec(&trimmed)?;
    if body.len() <= ENCODE_CAP {
        Ok(EncodedRequest {
            body,
            truncated_untrusted: true,
        })
    } else {
        Err(WireError::BodyCap(body.len()))
    }
}

pub fn check_response(
    questions: &IndexMap<String, WireQuestion>,
    response: &WireResponse,
) -> Result<(), DecodeError> {
    for (key, question) in questions {
        let Some(answer) = response.answers.get(key) else {
            return Err(DecodeError::MissingAnswer {
                key: QuestionId::new(key),
            });
        };
        match (question, answer) {
            (WireQuestion::Choice { criteria, .. }, WireAnswer::Choice { choice, .. }) => {
                if !criteria.contains_key(choice) {
                    return Err(DecodeError::UnknownLabel {
                        key: QuestionId::new(key),
                        label: choice.clone(),
                    });
                }
            }
            (WireQuestion::Noul { .. }, WireAnswer::Noul { noul }) => {
                if !(0.0..=1.0).contains(noul) {
                    return Err(DecodeError::OutOfRange {
                        key: QuestionId::new(key),
                    });
                }
            }
            (WireQuestion::Score { criteria, .. }, WireAnswer::Score { score, .. }) => {
                let max = (criteria.len() as f64) - 1.0;
                if *score < -1e-6 || *score > max + 1e-6 {
                    return Err(DecodeError::OutOfRange {
                        key: QuestionId::new(key),
                    });
                }
            }
            _ => {
                return Err(DecodeError::TypeMismatch {
                    key: QuestionId::new(key),
                });
            }
        }
    }
    Ok(())
}

pub fn renormalize_probabilities(
    probabilities: &IndexMap<String, f64>,
) -> (IndexMap<String, f64>, f64) {
    let original_sum: f64 = probabilities.values().sum();
    let off_one = (original_sum - 1.0).abs() > 1e-6;
    if original_sum.is_finite() && original_sum.abs() > 1e-12 && off_one {
        let scaled = probabilities
            .iter()
            .map(|(key, value)| (key.clone(), value / original_sum))
            .collect();
        (scaled, original_sum)
    } else {
        (probabilities.clone(), original_sum)
    }
}
