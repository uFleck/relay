//! JEV (TypeSafe System One): typed questions, typed answers, and routing.
//!
//! JEV answers a set of named questions about one piece of content (`state`).
//! Relay uses it as a fast filter in front of the reviewer (PLAN.md section 3.2):
//! stage A screens an episode, stage B matches it to candidate tasks, and
//! [`route`] turns the answers into a [`Route`] in code, never in the model.
//!
//! The wire contract is <https://docs.typesafe.ai/api/system-one.md>, with the
//! question and answer shapes from <https://docs.typesafe.ai/primitives>.
//!
//! The API key travels only in the `Authorization` header. It is never logged,
//! never included in an error, and redacted by [`JevClient`]'s `Debug`.

use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

/// The model alias the API documents in its examples.
pub const DEFAULT_MODEL: &str = "jev-latest";
/// The public origin of the TypeSafe API.
pub const DEFAULT_BASE_URL: &str = "https://api.typesafe.ai";
/// Requests give up after this many seconds unless [`JevConfig`] says otherwise.
pub const DEFAULT_TIMEOUT_SECS: u64 = 15;

/// The one documented endpoint for mixed typed questions.
const SYSTEM_ONE_PATH: &str = "/v1/systemone";
/// An error body is cut here, so one bad response cannot flood a log.
const MAX_ERROR_BODY_CHARS: usize = 500;

/// Answer IDs asked in PLAN.md section 3.2. Question IDs are for our code; the
/// model only sees `instructions`.
pub mod ids {
    /// Stage A: Noul, the message could affect work.
    pub const WORTH_REVIEWING: &str = "worth_reviewing";
    /// Stage A: Choice, `ongoing` / `waiting_on_me` / `resolved` / `abandoned`.
    pub const CONVERSATION_STATE: &str = "conversation_state";
    /// Stage B: Noul, changes a listed task.
    pub const AFFECTS_EXISTING: &str = "affects_existing";
    /// Stage B: Choice, a candidate id, `none` or `multiple`.
    pub const TASK: &str = "task";
    /// Stage B: Choice, what kind of change this is.
    pub const CHANGE_KIND: &str = "change_kind";
    /// Stage B: Noul, asks for work no listed task covers.
    pub const IS_NEW_TASK: &str = "is_new_task";
    /// Stage B: Noul, the conversation reached a decision or natural end.
    pub const CONVERSATION_COMPLETE: &str = "conversation_complete";
    /// Stage B: Score, can wait / this week / today-blocking.
    pub const URGENCY: &str = "urgency";
    /// Stage B: Noul, ambiguous, conflicting or risky to auto-apply.
    pub const NEEDS_HUMAN: &str = "needs_human";
    /// Stage B: Choice, project slugs plus `unsure`.
    pub const PROJECT: &str = "project";

    /// The `task` choice that means "no listed task".
    pub const TASK_NONE: &str = "none";
    /// The `task` choice that means "more than one listed task".
    pub const TASK_MULTIPLE: &str = "multiple";
    /// The `project` choice that means "let the reviewer pick".
    pub const PROJECT_UNSURE: &str = "unsure";
}

/// Connection settings. An empty `model` or `base_url`, or a zero timeout,
/// falls back to the defaults in [`JevClient::with_config`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct JevConfig {
    /// Model name or alias; the response reports the concrete model.
    pub model: String,
    /// Origin only, no trailing slash and no path.
    pub base_url: String,
    /// Whole-request timeout.
    pub timeout_secs: u64,
}

impl Default for JevConfig {
    fn default() -> Self {
        Self {
            model: DEFAULT_MODEL.to_string(),
            base_url: DEFAULT_BASE_URL.to_string(),
            timeout_secs: DEFAULT_TIMEOUT_SECS,
        }
    }
}

/// The API key, kept out of `Debug` even if a future type derives it.
#[derive(Clone)]
struct ApiKey(String);

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// Everything that can go wrong. No variant carries the API key.
#[derive(Debug, Error)]
pub enum JevError {
    /// The configured key is empty or whitespace only.
    #[error("missing JEV API key")]
    MissingKey,
    /// Transport failure: DNS, connect, TLS, a bad URL, a broken body.
    #[error("JEV request failed: {0}")]
    Http(#[from] reqwest::Error),
    /// The API answered with a non-2xx status. `body` is capped at 500 chars.
    #[error("JEV returned HTTP {status}: {body}")]
    Status { status: u16, body: String },
    /// A 2xx body that is not the documented response shape.
    #[error("could not decode the JEV response: {0}")]
    Decode(#[from] serde_json::Error),
    /// The request exceeded [`JevConfig::timeout_secs`].
    #[error("JEV request timed out")]
    Timeout,
}

/// A System One client. Cheap to clone; the inner `reqwest::Client` pools.
#[derive(Clone)]
pub struct JevClient {
    api_key: ApiKey,
    config: JevConfig,
    http: reqwest::Client,
    endpoint: String,
}

impl fmt::Debug for JevClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JevClient")
            .field("api_key", &self.api_key)
            .field("model", &self.config.model)
            .field("base_url", &self.config.base_url)
            .field("timeout_secs", &self.config.timeout_secs)
            .finish_non_exhaustive()
    }
}

impl JevClient {
    /// A client with [`JevConfig::default`]. Fails on an empty key or if the
    /// HTTP stack cannot initialize.
    pub fn new(api_key: String) -> Result<Self, JevError> {
        Self::with_config(api_key, JevConfig::default())
    }

    /// A client with explicit settings.
    ///
    /// The key is trimmed and checked here; a missing key never reaches the
    /// network. Empty config values fall back to defaults.
    pub fn with_config(api_key: String, config: JevConfig) -> Result<Self, JevError> {
        let api_key = api_key.trim();
        if api_key.is_empty() {
            return Err(JevError::MissingKey);
        }

        let mut config = config;
        config.model = config.model.trim().to_string();
        if config.model.is_empty() {
            config.model = DEFAULT_MODEL.to_string();
        }
        config.base_url = config.base_url.trim().trim_end_matches('/').to_string();
        if config.base_url.is_empty() {
            config.base_url = DEFAULT_BASE_URL.to_string();
        }
        if config.timeout_secs == 0 {
            config.timeout_secs = DEFAULT_TIMEOUT_SECS;
        }

        // reqwest 0.13 asks the process for a rustls provider when built with
        // `rustls-no-provider`; install the ring provider only when absent, so
        // the binary's own choice (the updater uses ring too) wins.
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }

        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()?;
        let endpoint = format!("{}{SYSTEM_ONE_PATH}", config.base_url);

        Ok(Self {
            api_key: ApiKey(api_key.to_string()),
            config,
            http,
            endpoint,
        })
    }

    /// The settings in use. The key is not part of [`JevConfig`].
    pub fn config(&self) -> &JevConfig {
        &self.config
    }

    /// Ask every question about `state` in one request. Answers come back under
    /// the same ids, with the concrete model and token usage.
    ///
    /// `state` is any JSON the API accepts: a string, an object or an array.
    /// Questions are answered independently, so ask speculative ones too.
    pub async fn evaluate(
        &self,
        state: Value,
        questions: BTreeMap<String, Question>,
    ) -> Result<JevResponse, JevError> {
        let request = SystemOneRequest {
            model: &self.config.model,
            state: &state,
            questions: &questions,
        };

        let response = self
            .http
            .post(&self.endpoint)
            .bearer_auth(&self.api_key.0)
            .json(&request)
            .send()
            .await
            .map_err(map_reqwest)?;

        let status = response.status();
        let body = response.text().await.map_err(map_reqwest)?;
        if !status.is_success() {
            return Err(JevError::Status {
                status: status.as_u16(),
                body: truncate_body(&body),
            });
        }

        serde_json::from_str(&body).map_err(JevError::Decode)
    }
}

fn map_reqwest(error: reqwest::Error) -> JevError {
    if error.is_timeout() {
        JevError::Timeout
    } else {
        JevError::Http(error)
    }
}

/// Longest valid UTF-8 prefix of at most [`MAX_ERROR_BODY_CHARS`] characters.
fn truncate_body(body: &str) -> String {
    match body.char_indices().nth(MAX_ERROR_BODY_CHARS) {
        Some((index, _)) => body[..index].to_string(),
        None => body.to_string(),
    }
}

#[derive(Serialize)]
struct SystemOneRequest<'a> {
    model: &'a str,
    state: &'a Value,
    questions: &'a BTreeMap<String, Question>,
}

// ---------------------------------------------------------------------------
// Questions
// ---------------------------------------------------------------------------

/// A named question. Serialized internally tagged on `type`.
///
/// The API accepts 1..=255 choices and 1..=10 score levels; an out-of-range
/// question is rejected by the API with HTTP 422, surfaced as
/// [`JevError::Status`]. This client does not duplicate those limits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Question {
    /// Yes/no: the probability the statement is true.
    Noul {
        #[serde(skip_serializing_if = "Option::is_none")]
        instructions: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        criteria: Option<NoulCriteria>,
    },
    /// One of a fixed set of options.
    Choice {
        #[serde(skip_serializing_if = "Option::is_none")]
        instructions: Option<Value>,
        criteria: BTreeMap<String, Value>,
    },
    /// A position along ordered levels.
    Score {
        #[serde(skip_serializing_if = "Option::is_none")]
        instructions: Option<Value>,
        criteria: Vec<Value>,
    },
}

/// What counts as yes and no for a [`Question::Noul`]. Both sides are optional.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NoulCriteria {
    #[serde(rename = "true", skip_serializing_if = "Option::is_none")]
    pub yes: Option<Value>,
    #[serde(rename = "false", skip_serializing_if = "Option::is_none")]
    pub no: Option<Value>,
}

impl Question {
    /// A Noul question. `Value::Null` omits `instructions`.
    pub fn noul(instructions: impl Into<Value>) -> Self {
        Self::Noul {
            instructions: optional_instructions(instructions),
            criteria: None,
        }
    }

    /// A Noul question with explicit yes/no criteria.
    pub fn noul_with_criteria(instructions: impl Into<Value>, criteria: NoulCriteria) -> Self {
        Self::Noul {
            instructions: optional_instructions(instructions),
            criteria: Some(criteria),
        }
    }

    /// A Choice question. Criteria keys are the choice names.
    pub fn choice<I, K, V>(instructions: impl Into<Value>, criteria: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<Value>,
    {
        Self::Choice {
            instructions: optional_instructions(instructions),
            criteria: criteria
                .into_iter()
                .map(|(name, description)| (name.into(), description.into()))
                .collect(),
        }
    }

    /// A Score question. Level order is the score: the first level is 0.
    pub fn score(
        instructions: impl Into<Value>,
        levels: impl IntoIterator<Item = impl Into<Value>>,
    ) -> Self {
        Self::Score {
            instructions: optional_instructions(instructions),
            criteria: levels.into_iter().map(Into::into).collect(),
        }
    }
}

fn optional_instructions(instructions: impl Into<Value>) -> Option<Value> {
    let value = instructions.into();
    (!value.is_null()).then_some(value)
}

// ---------------------------------------------------------------------------
// Responses
// ---------------------------------------------------------------------------

/// The documented response: the concrete model, answers by question id, usage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JevResponse {
    /// May differ from the alias sent in the request.
    pub model: String,
    /// Keyed by the ids from the request.
    pub answers: BTreeMap<String, Answer>,
    pub usage: Usage,
}

/// Billable input and (currently free) output tokens for one request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// One typed answer, tagged on `type`. Each payload is a named struct so the
/// accessors can hand out references.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Answer {
    Noul(NoulAnswer),
    Choice(ChoiceAnswer),
    Score(ScoreAnswer),
}

/// Probability of yes, from 0 to 1.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NoulAnswer {
    pub noul: f64,
}

/// The selected option and the distribution over every option.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChoiceAnswer {
    /// One of the criteria names.
    pub choice: String,
    /// How peaked the selection is, from 0 to 1.
    pub confidence: f64,
    /// Probability of each criteria name, from 0 to 1.
    pub probabilities: BTreeMap<String, f64>,
}

/// The expected score, its rubric, and the distribution over levels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScoreAnswer {
    /// Probability-weighted average of the levels; may be fractional.
    pub score: f64,
    /// How peaked the distribution is, from 0 to 1.
    pub confidence: f64,
    /// The requested levels keyed by `"0"`, `"1"`, ...
    pub legend: BTreeMap<String, Value>,
    /// Probability of each level, same keys as `legend`.
    pub probabilities: BTreeMap<String, f64>,
}

impl JevResponse {
    /// The typed answer under `id`, whatever its type.
    pub fn answer(&self, id: &str) -> Option<&Answer> {
        self.answers.get(id)
    }

    /// The Noul probability under `id`, when that id is a Noul.
    pub fn noul(&self, id: &str) -> Option<f64> {
        match self.answer(id)? {
            Answer::Noul(answer) => Some(answer.noul),
            Answer::Choice(_) | Answer::Score(_) => None,
        }
    }

    /// The Choice payload under `id`, when that id is a Choice.
    pub fn choice(&self, id: &str) -> Option<&ChoiceAnswer> {
        match self.answer(id)? {
            Answer::Choice(answer) => Some(answer),
            Answer::Noul(_) | Answer::Score(_) => None,
        }
    }

    /// The Score payload under `id`, when that id is a Score.
    pub fn score(&self, id: &str) -> Option<&ScoreAnswer> {
        match self.answer(id)? {
            Answer::Score(answer) => Some(answer),
            Answer::Noul(_) | Answer::Choice(_) => None,
        }
    }

    /// Token usage for the request.
    pub fn usage(&self) -> Usage {
        self.usage
    }
}

// ---------------------------------------------------------------------------
// Routing (PLAN.md section 3.2)
// ---------------------------------------------------------------------------

/// Per-project routing thresholds. Defaults are the PLAN table; `>` applies
/// everywhere, so a value exactly on a threshold does not trigger it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Thresholds {
    /// Stage A: above this the episode is worth stage B.
    pub screen_yes: f64,
    /// Stage A: below this the episode is archived.
    pub screen_no: f64,
    /// `affects_existing` above this is a change.
    pub change: f64,
    /// `is_new_task` above this, with `complete`, creates a task.
    pub new_task: f64,
    /// `conversation_complete` above this, with `new_task`, creates a task.
    pub complete: f64,
    /// `needs_human` above this always reaches the review queue.
    pub needs_human: f64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            screen_yes: 0.60,
            screen_no: 0.25,
            change: 0.70,
            new_task: 0.80,
            complete: 0.70,
            needs_human: 0.60,
        }
    }
}

/// What Relay does with a set of answers. Routing lives in code, not in JEV.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Route {
    /// Archive: the episode is not about work.
    Ignore,
    /// A human reads it. Every uncertain, conflicting or incomplete case.
    Review { reason: String },
    /// Append to an existing task. `early` only when the task is claimed.
    Change {
        task_id: Option<String>,
        early: bool,
    },
    /// Create a task. `None` means the reviewer picks (project was `unsure`).
    NewTask { project: Option<String> },
}

/// Turn one response into a [`Route`] (PLAN.md section 3.2).
///
/// Order matters:
///
/// 1. `worth_reviewing < screen_no` archives. A missing screen answer never does.
/// 2. `worth_reviewing` in `[screen_no, screen_yes]` is the middle band.
/// 3. `needs_human > needs_human` always reviews.
/// 4. `affects_existing` and `is_new_task` both high reviews; the reviewer decides.
/// 5. `task = multiple` reviews; the reviewer splits one change per task.
/// 6. `affects_existing > change` with a task appends, early only if claimed;
///    with `task = none` the answers conflict and it reviews.
/// 7. `is_new_task > new_task` and `conversation_complete > complete` creates.
/// 8. Anything else, and any missing answer, reviews. Never auto-apply on
///    incomplete answers.
pub fn route(response: &JevResponse, thresholds: Thresholds, task_claimed: bool) -> Route {
    let Some(worth_reviewing) = response.noul(ids::WORTH_REVIEWING) else {
        return review(format!("missing {}", ids::WORTH_REVIEWING));
    };
    if worth_reviewing < thresholds.screen_no {
        return Route::Ignore;
    }
    if worth_reviewing <= thresholds.screen_yes {
        return review(format!(
            "{} {worth_reviewing:.2} is in the middle band; rescreen at episode close",
            ids::WORTH_REVIEWING
        ));
    }

    let Some(needs_human) = response.noul(ids::NEEDS_HUMAN) else {
        return review(format!("missing {}", ids::NEEDS_HUMAN));
    };
    if needs_human > thresholds.needs_human {
        return review(format!(
            "{} {needs_human:.2} is above {:.2}",
            ids::NEEDS_HUMAN,
            thresholds.needs_human
        ));
    }

    let Some(affects_existing) = response.noul(ids::AFFECTS_EXISTING) else {
        return review(format!("missing {}", ids::AFFECTS_EXISTING));
    };
    let Some(is_new_task) = response.noul(ids::IS_NEW_TASK) else {
        return review(format!("missing {}", ids::IS_NEW_TASK));
    };
    let Some(task) = response.choice(ids::TASK) else {
        return review(format!("missing {}", ids::TASK));
    };

    if affects_existing > thresholds.change && is_new_task > thresholds.new_task {
        return review(format!(
            "{} {affects_existing:.2} and {} {is_new_task:.2} are both high",
            ids::AFFECTS_EXISTING,
            ids::IS_NEW_TASK
        ));
    }

    if task.choice == ids::TASK_MULTIPLE {
        return review(format!(
            "{} is multiple; the reviewer splits one change per task",
            ids::TASK
        ));
    }

    if affects_existing > thresholds.change {
        if task.choice == ids::TASK_NONE {
            return review(format!(
                "{} {affects_existing:.2} is high but {} is none; the answers conflict",
                ids::AFFECTS_EXISTING,
                ids::TASK
            ));
        }
        return Route::Change {
            task_id: Some(task.choice.clone()),
            early: task_claimed,
        };
    }

    let Some(conversation_complete) = response.noul(ids::CONVERSATION_COMPLETE) else {
        return review(format!("missing {}", ids::CONVERSATION_COMPLETE));
    };
    if is_new_task > thresholds.new_task && conversation_complete > thresholds.complete {
        let Some(project) = response.choice(ids::PROJECT) else {
            return review(format!("missing {}", ids::PROJECT));
        };
        let project = (project.choice != ids::PROJECT_UNSURE).then(|| project.choice.clone());
        return Route::NewTask { project };
    }

    review(format!(
        "{} {affects_existing:.2} and {} {is_new_task:.2} are in the middle band; \
         rescreen at episode close",
        ids::AFFECTS_EXISTING,
        ids::IS_NEW_TASK
    ))
}

fn review(reason: impl Into<String>) -> Route {
    Route::Review {
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use axum::Router;
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode, header};
    use axum::routing::post;
    use serde_json::{Value, json};
    use tokio::net::TcpListener;
    use tokio::sync::Mutex;

    use super::*;

    const TEST_KEY: &str = "jev-test-key-do-not-log";

    // -----------------------------------------------------------------------
    // Mock server
    // -----------------------------------------------------------------------

    #[derive(Debug, Clone)]
    struct CapturedRequest {
        authorization: Option<String>,
        content_type: Option<String>,
        body: Value,
    }

    #[derive(Clone)]
    struct MockState {
        status: StatusCode,
        body: String,
        delay: Option<Duration>,
        captured: Arc<Mutex<Option<CapturedRequest>>>,
    }

    struct MockServer {
        base_url: String,
        captured: Arc<Mutex<Option<CapturedRequest>>>,
    }

    impl MockServer {
        async fn start(status: StatusCode, body: String) -> Self {
            Self::start_slow(status, body, None).await
        }

        async fn start_slow(status: StatusCode, body: String, delay: Option<Duration>) -> Self {
            let captured = Arc::new(Mutex::new(None));
            let state = MockState {
                status,
                body,
                delay,
                captured: Arc::clone(&captured),
            };

            let router = Router::new()
                .route(SYSTEM_ONE_PATH, post(capture))
                .with_state(state);
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base_url = format!("http://{}", listener.local_addr().unwrap());
            tokio::spawn(async move {
                axum::serve(listener, router).await.unwrap();
            });

            Self { base_url, captured }
        }

        async fn captured(&self) -> CapturedRequest {
            self.captured
                .lock()
                .await
                .clone()
                .expect("the client sent a request")
        }

        fn client(&self) -> JevClient {
            client(&self.base_url)
        }
    }

    async fn capture(
        State(state): State<MockState>,
        headers: HeaderMap,
        body: String,
    ) -> (StatusCode, String) {
        let authorization = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let content_type = headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        *state.captured.lock().await = Some(CapturedRequest {
            authorization,
            content_type,
            body: serde_json::from_str(&body).unwrap_or(Value::Null),
        });
        if let Some(delay) = state.delay {
            tokio::time::sleep(delay).await;
        }
        (state.status, state.body)
    }

    fn client(base_url: &str) -> JevClient {
        JevClient::with_config(
            TEST_KEY.to_string(),
            JevConfig {
                model: "jev-test".to_string(),
                base_url: base_url.to_string(),
                timeout_secs: 5,
            },
        )
        .unwrap()
    }

    fn questions() -> BTreeMap<String, Question> {
        BTreeMap::from([
            (
                ids::WORTH_REVIEWING.to_string(),
                Question::noul("Does the message ask for work?"),
            ),
            (
                ids::TASK.to_string(),
                Question::choice(
                    "Which task does the message affect?",
                    [("task_1", "The deploy task"), ("none", "No task")],
                ),
            ),
            (
                ids::URGENCY.to_string(),
                Question::score(
                    "How urgent is the message?",
                    ["Can wait", "This week", "Today"],
                ),
            ),
        ])
    }

    fn response_body() -> String {
        json!({
            "model": "jev-test-2026-01-01",
            "answers": {
                "worth_reviewing": {"type": "noul", "noul": 0.82},
                "task": {
                    "type": "choice",
                    "choice": "task_1",
                    "confidence": 0.91,
                    "probabilities": {"task_1": 0.91, "none": 0.09}
                },
                "urgency": {
                    "type": "score",
                    "score": 1.7,
                    "confidence": 0.7,
                    "legend": {"0": "Can wait", "1": "This week", "2": "Today"},
                    "probabilities": {"0": 0.1, "1": 0.2, "2": 0.7}
                }
            },
            "usage": {"input_tokens": 321, "output_tokens": 12}
        })
        .to_string()
    }

    // -----------------------------------------------------------------------
    // 1. Request shape and response parsing
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn evaluate_sends_the_documented_request_and_parses_every_answer_type() {
        let server = MockServer::start(StatusCode::OK, response_body()).await;
        let jev = server.client();
        let state = json!({"messages": [{"author": "Ana", "text": "Also needs the RC build."}]});

        let response = jev.evaluate(state.clone(), questions()).await.unwrap();

        assert_eq!(response.model, "jev-test-2026-01-01");
        assert_eq!(response.noul(ids::WORTH_REVIEWING), Some(0.82));
        let task = response.choice(ids::TASK).expect("a choice answer");
        assert_eq!(task.choice, "task_1");
        assert_eq!(task.confidence, 0.91);
        assert_eq!(task.probabilities.get("none"), Some(&0.09));
        let urgency = response.score(ids::URGENCY).expect("a score answer");
        assert_eq!(urgency.score, 1.7);
        assert_eq!(urgency.legend.get("2"), Some(&json!("Today")));
        assert_eq!(
            response.usage(),
            Usage {
                input_tokens: 321,
                output_tokens: 12,
            }
        );

        // An id of the wrong type is None, not a wrong value.
        assert_eq!(response.noul(ids::TASK), None);
        assert!(response.choice(ids::WORTH_REVIEWING).is_none());
        assert!(response.score("missing").is_none());

        let captured = server.captured().await;
        assert_eq!(
            captured.authorization.as_deref(),
            Some(format!("Bearer {TEST_KEY}").as_str())
        );
        assert!(
            captured
                .content_type
                .as_deref()
                .is_some_and(|value| value.starts_with("application/json"))
        );
        assert_eq!(captured.body["model"], json!("jev-test"));
        assert_eq!(captured.body["state"], state);
        assert_eq!(
            captured.body["questions"][ids::WORTH_REVIEWING],
            json!({"type": "noul", "instructions": "Does the message ask for work?"})
        );
        assert_eq!(
            captured.body["questions"][ids::TASK]["criteria"]["task_1"],
            json!("The deploy task")
        );
        assert_eq!(
            captured.body["questions"][ids::URGENCY]["criteria"],
            json!(["Can wait", "This week", "Today"])
        );
    }

    #[test]
    fn questions_serialize_to_the_documented_shape() {
        let noul = serde_json::to_value(Question::noul("Is this billing?")).unwrap();
        assert_eq!(
            noul,
            json!({"type": "noul", "instructions": "Is this billing?"})
        );

        let with_criteria = serde_json::to_value(Question::noul_with_criteria(
            "Is this spam?",
            NoulCriteria {
                yes: Some(json!("Unsolicited advertising")),
                no: Some(json!("A legitimate conversation")),
            },
        ))
        .unwrap();
        assert_eq!(
            with_criteria["criteria"],
            json!({"true": "Unsolicited advertising", "false": "A legitimate conversation"})
        );

        let choice = serde_json::to_value(Question::choice(
            "What tone?",
            [("calm", "Polite"), ("angry", "Hostile")],
        ))
        .unwrap();
        assert_eq!(choice["type"], json!("choice"));
        assert_eq!(choice["criteria"]["calm"], json!("Polite"));

        let score = serde_json::to_value(Question::score(
            "How urgent?",
            ["Can wait", "This week", "Today"],
        ))
        .unwrap();
        assert_eq!(score["type"], json!("score"));
        assert_eq!(score["criteria"], json!(["Can wait", "This week", "Today"]));

        // Null instructions are omitted rather than sent as null.
        let omitted = serde_json::to_value(Question::noul(Value::Null)).unwrap();
        assert_eq!(omitted, json!({"type": "noul"}));
    }

    // -----------------------------------------------------------------------
    // 2. HTTP errors and timeouts
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn a_400_is_a_status_error_with_a_capped_body() {
        let long_body = format!(r#"{{"detail":"{}"}}"#, "x".repeat(600));
        let server = MockServer::start(StatusCode::BAD_REQUEST, long_body).await;

        let error = server
            .client()
            .evaluate(json!("hello"), questions())
            .await
            .unwrap_err();

        match &error {
            JevError::Status { status, body } => {
                assert_eq!(*status, 400);
                assert_eq!(body.chars().count(), 500);
            }
            other => panic!("expected Status, got {other:?}"),
        }
        assert!(!format!("{error}").contains(TEST_KEY));
    }

    #[tokio::test]
    async fn a_slow_server_is_a_timeout() {
        let server = MockServer::start_slow(
            StatusCode::OK,
            response_body(),
            Some(Duration::from_secs(2)),
        )
        .await;
        let jev = JevClient::with_config(
            TEST_KEY.to_string(),
            JevConfig {
                base_url: server.base_url.clone(),
                timeout_secs: 1,
                ..JevConfig::default()
            },
        )
        .unwrap();

        let error = jev.evaluate(json!("hello"), questions()).await.unwrap_err();
        assert!(matches!(error, JevError::Timeout), "got {error:?}");
    }

    // -----------------------------------------------------------------------
    // 3. Decode failures
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn a_2xx_that_is_not_json_is_a_decode_error() {
        let server = MockServer::start(StatusCode::OK, "not json".to_string()).await;

        let error = server
            .client()
            .evaluate(json!("hello"), questions())
            .await
            .unwrap_err();

        assert!(matches!(error, JevError::Decode(_)), "got {error:?}");
        assert!(!format!("{error}").contains(TEST_KEY));
    }

    #[tokio::test]
    async fn a_response_missing_usage_is_a_decode_error() {
        let server = MockServer::start(
            StatusCode::OK,
            json!({"model": "jev-latest", "answers": {}}).to_string(),
        )
        .await;

        let error = server
            .client()
            .evaluate(json!("hello"), questions())
            .await
            .unwrap_err();

        assert!(matches!(error, JevError::Decode(_)), "got {error:?}");
    }

    // -----------------------------------------------------------------------
    // 4. Routing table
    // -----------------------------------------------------------------------

    fn response(answers: Value) -> JevResponse {
        serde_json::from_value(json!({
            "model": "jev-latest",
            "answers": answers,
            "usage": {"input_tokens": 100, "output_tokens": 5}
        }))
        .unwrap()
    }

    fn noul(value: f64) -> Value {
        json!({"type": "noul", "noul": value})
    }

    fn choice(value: &str) -> Value {
        json!({
            "type": "choice",
            "choice": value,
            "confidence": 0.9,
            "probabilities": {}
        })
    }

    fn score(value: f64) -> Value {
        json!({
            "type": "score",
            "score": value,
            "confidence": 0.5,
            "legend": {"0": "Can wait", "1": "This week", "2": "Today"},
            "probabilities": {"0": 0.5, "1": 0.3, "2": 0.2}
        })
    }

    /// A complete stage-B answer set with the routing-relevant values filled in.
    fn stage_b(
        worth_reviewing: f64,
        needs_human: f64,
        affects_existing: f64,
        task: &str,
        is_new_task: f64,
        complete: f64,
    ) -> Value {
        json!({
            "worth_reviewing": noul(worth_reviewing),
            "conversation_state": choice("ongoing"),
            "needs_human": noul(needs_human),
            "affects_existing": noul(affects_existing),
            "task": choice(task),
            "change_kind": choice("add_requirement"),
            "is_new_task": noul(is_new_task),
            "conversation_complete": noul(complete),
            "urgency": score(1.0),
            "project": choice("relay")
        })
    }

    fn review_reason(route: &Route) -> &str {
        match route {
            Route::Review { reason } => reason,
            other => panic!("expected Review, got {other:?}"),
        }
    }

    #[test]
    fn routing_screen_archives_and_rescreens() {
        let thresholds = Thresholds::default();

        // Below screen_no archives even when every other answer is loud.
        let loud = stage_b(0.10, 0.99, 0.99, "task_1", 0.99, 0.99);
        assert_eq!(route(&response(loud), thresholds, true), Route::Ignore);

        // Exactly at screen_no is not below it: the middle band reviews.
        let boundary = stage_b(0.25, 0.10, 0.10, "none", 0.10, 0.10);
        assert!(matches!(
            route(&response(boundary), thresholds, false),
            Route::Review { .. }
        ));

        // Between screen_no and screen_yes: rescreen at close.
        let middle = stage_b(0.45, 0.10, 0.10, "none", 0.10, 0.10);
        let decision = route(&response(middle), thresholds, false);
        assert!(
            review_reason(&decision).contains("middle band"),
            "{decision:?}"
        );
    }

    #[test]
    fn routing_change_is_early_only_when_the_task_is_claimed() {
        let thresholds = Thresholds::default();
        let answers = stage_b(0.90, 0.10, 0.85, "task_1", 0.10, 0.90);

        assert_eq!(
            route(&response(answers.clone()), thresholds, true),
            Route::Change {
                task_id: Some("task_1".to_string()),
                early: true
            }
        );
        assert_eq!(
            route(&response(answers), thresholds, false),
            Route::Change {
                task_id: Some("task_1".to_string()),
                early: false
            }
        );
    }

    #[test]
    fn routing_new_task_needs_completion_and_maps_unsure_to_none() {
        let thresholds = Thresholds::default();

        let mut answers = stage_b(0.90, 0.10, 0.20, "none", 0.90, 0.90);
        assert_eq!(
            route(&response(answers.clone()), thresholds, false),
            Route::NewTask {
                project: Some("relay".to_string())
            }
        );

        answers["project"] = choice(ids::PROJECT_UNSURE);
        assert_eq!(
            route(&response(answers.clone()), thresholds, false),
            Route::NewTask { project: None }
        );

        // A new task from an open conversation waits for the close.
        let open = stage_b(0.90, 0.10, 0.20, "none", 0.90, 0.40);
        let decision = route(&response(open), thresholds, false);
        assert!(
            review_reason(&decision).contains("middle band"),
            "{decision:?}"
        );
    }

    #[test]
    fn routing_needs_human_multiple_and_both_high_review() {
        let thresholds = Thresholds::default();

        let needs_human = stage_b(0.90, 0.80, 0.85, "task_1", 0.10, 0.90);
        let decision = route(&response(needs_human), thresholds, false);
        assert!(
            review_reason(&decision).contains("needs_human"),
            "{decision:?}"
        );

        let multiple = stage_b(0.90, 0.10, 0.85, ids::TASK_MULTIPLE, 0.10, 0.90);
        let decision = route(&response(multiple), thresholds, false);
        assert!(
            review_reason(&decision).contains("multiple"),
            "{decision:?}"
        );

        let both = stage_b(0.90, 0.10, 0.90, "task_1", 0.90, 0.90);
        let decision = route(&response(both), thresholds, false);
        assert!(
            review_reason(&decision).contains("both high"),
            "{decision:?}"
        );
    }

    #[test]
    fn routing_never_auto_applies_on_missing_or_middle_answers() {
        let thresholds = Thresholds::default();

        // No stage A answer at all.
        let empty = route(&response(json!({})), thresholds, false);
        assert!(
            review_reason(&empty).contains(ids::WORTH_REVIEWING),
            "{empty:?}"
        );

        // Stage A said yes, but stage B was not answered.
        let stage_a_only = route(
            &response(json!({"worth_reviewing": noul(0.90)})),
            thresholds,
            false,
        );
        assert!(
            review_reason(&stage_a_only).contains(ids::NEEDS_HUMAN),
            "{stage_a_only:?}"
        );

        // A high new-task signal without a project cannot pick a destination.
        let mut no_project = stage_b(0.90, 0.10, 0.20, "none", 0.90, 0.90);
        no_project.as_object_mut().unwrap().remove(ids::PROJECT);
        let decision = route(&response(no_project), thresholds, false);
        assert!(
            review_reason(&decision).contains("missing project"),
            "{decision:?}"
        );

        // affects_existing high but the task is none: conflicting, so review.
        let conflicting = stage_b(0.90, 0.10, 0.85, ids::TASK_NONE, 0.10, 0.90);
        let decision = route(&response(conflicting), thresholds, false);
        assert!(
            review_reason(&decision).contains("conflict"),
            "{decision:?}"
        );
    }

    #[test]
    fn routing_thresholds_are_strict_and_configurable() {
        // Exactly on the threshold does not trigger it.
        let exact = stage_b(0.90, 0.60, 0.70, "task_1", 0.80, 0.70);
        let decision = route(&response(exact), Thresholds::default(), false);
        assert!(
            review_reason(&decision).contains("middle band"),
            "{decision:?}"
        );

        // A raised needs_human ceiling lets a change through.
        let custom = Thresholds {
            needs_human: 0.95,
            ..Thresholds::default()
        };
        let answers = stage_b(0.90, 0.80, 0.85, "task_1", 0.10, 0.90);
        assert_eq!(
            route(&response(answers), custom, false),
            Route::Change {
                task_id: Some("task_1".to_string()),
                early: false
            }
        );
    }

    #[test]
    fn answers_round_trip_with_their_type_tag() {
        let samples = [
            json!({"type": "noul", "noul": 0.98}),
            json!({"type": "choice", "choice": "calm", "confidence": 0.7,
                   "probabilities": {"calm": 0.8, "angry": 0.2}}),
            json!({"type": "score", "score": 1.7, "confidence": 0.55,
                   "legend": {"0": "Can wait"}, "probabilities": {"0": 0.3}}),
        ];

        for raw in samples {
            let answer: Answer = serde_json::from_value(raw.clone()).unwrap();
            assert_eq!(serde_json::to_value(&answer).unwrap(), raw);
        }
    }

    // -----------------------------------------------------------------------
    // 5. Secrets
    // -----------------------------------------------------------------------

    #[test]
    fn the_client_debug_redacts_the_key() {
        let jev = JevClient::new(TEST_KEY.to_string()).unwrap();
        let debug = format!("{jev:?}");

        assert!(!debug.contains(TEST_KEY), "{debug}");
        assert!(debug.contains("redacted"), "{debug}");
        assert!(debug.contains("jev-latest"), "{debug}");
    }

    #[test]
    fn an_empty_key_is_rejected_before_any_request() {
        assert!(matches!(
            JevClient::new(String::new()),
            Err(JevError::MissingKey)
        ));
        assert!(matches!(
            JevClient::new("   \t\n".to_string()),
            Err(JevError::MissingKey)
        ));
        assert!(!format!("{}", JevError::MissingKey).contains(TEST_KEY));
    }

    #[test]
    fn config_defaults_and_normalization() {
        let defaults = JevConfig::default();
        assert_eq!(defaults.model, "jev-latest");
        assert_eq!(defaults.base_url, "https://api.typesafe.ai");
        assert_eq!(defaults.timeout_secs, 15);

        let jev = JevClient::with_config(
            TEST_KEY.to_string(),
            JevConfig {
                model: "  ".to_string(),
                base_url: " http://127.0.0.1:1/ ".to_string(),
                timeout_secs: 0,
            },
        )
        .unwrap();
        let config = jev.config();
        assert_eq!(config.model, "jev-latest");
        assert_eq!(config.base_url, "http://127.0.0.1:1");
        assert_eq!(config.timeout_secs, 15);
    }
}
