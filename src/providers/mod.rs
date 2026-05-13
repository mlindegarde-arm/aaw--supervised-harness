use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::future::Future;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::domain::{OllamaConfig, OpenAiConfig};
use crate::security::{DefaultProviderUrlPolicy, DefaultRedactor, ProviderUrlPolicy, Redactor};

#[derive(Debug, Clone, PartialEq)]
pub struct ModelRequest {
    pub model: String,
    pub system: Option<String>,
    pub input: String,
    pub temperature: Option<f32>,
    pub max_output_tokens: Option<u32>,
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelResponse {
    pub provider: String,
    pub model: String,
    pub response_id: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderErrorKind {
    AuthFailed,
    RateLimited,
    Timeout,
    HttpServer,
    BadRequest,
    ModelMissing,
    InvalidJson,
    IncompleteResponse,
    EmptyOutput,
}

impl ProviderErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AuthFailed => "auth_failed",
            Self::RateLimited => "rate_limited",
            Self::Timeout => "timeout",
            Self::HttpServer => "http_server",
            Self::BadRequest => "bad_request",
            Self::ModelMissing => "model_missing",
            Self::InvalidJson => "invalid_json",
            Self::IncompleteResponse => "incomplete_response",
            Self::EmptyOutput => "empty_output",
        }
    }

    pub fn is_retryable(self) -> bool {
        matches!(self, Self::RateLimited | Self::Timeout | Self::HttpServer)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderError {
    pub kind: ProviderErrorKind,
    pub message: String,
}

pub type ProviderResult<T> = Result<T, ProviderError>;
pub type ProviderFuture<'a, T> = Pin<Box<dyn Future<Output = ProviderResult<T>> + Send + 'a>>;

pub trait ModelProvider: Send + Sync {
    fn complete<'a>(&'a self, request: ModelRequest) -> ProviderFuture<'a, ModelResponse>;
}

impl ProviderError {
    pub fn new(kind: ProviderErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn is_retryable(&self) -> bool {
        self.kind.is_retryable()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    pub max_retries: u32,
    pub backoff: Duration,
}

impl RetryPolicy {
    pub fn new(max_retries: u32, backoff: Duration) -> Self {
        Self {
            max_retries,
            backoff,
        }
    }

    pub fn max_attempts(self) -> u32 {
        self.max_retries.saturating_add(1)
    }

    pub fn should_retry(self, error: &ProviderError, completed_retries: u32) -> bool {
        error.is_retryable() && completed_retries < self.max_retries
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProviderHttpSettings {
    connect_timeout: Duration,
    timeout: Duration,
    retry_policy: RetryPolicy,
}

impl ProviderHttpSettings {
    fn new(
        connect_timeout_seconds: u64,
        timeout_seconds: u64,
        max_retries: u32,
        retry_backoff_ms: u64,
    ) -> Self {
        Self {
            connect_timeout: Duration::from_secs(connect_timeout_seconds),
            timeout: Duration::from_secs(timeout_seconds),
            retry_policy: RetryPolicy::new(max_retries, Duration::from_millis(retry_backoff_ms)),
        }
    }

    fn agent(self) -> ureq::Agent {
        ureq::AgentBuilder::new()
            .timeout_connect(self.connect_timeout)
            .timeout(self.timeout)
            .redirects(0)
            .build()
    }
}

#[derive(Debug, Clone)]
pub struct OllamaProvider {
    base_url: String,
    keep_alive: String,
    num_ctx: u32,
    num_predict: u32,
    seed: u32,
    temperature: f32,
    http: ProviderHttpSettings,
}

impl OllamaProvider {
    pub fn new(config: &OllamaConfig) -> Self {
        Self {
            base_url: trim_base_url(&config.base_url),
            keep_alive: config.keep_alive.clone(),
            num_ctx: config.num_ctx,
            num_predict: config.num_predict,
            seed: config.seed,
            temperature: config.temperature,
            http: ProviderHttpSettings::new(
                config.connect_timeout_seconds,
                config.timeout_seconds,
                config.max_retries,
                config.retry_backoff_ms,
            ),
        }
    }

    pub fn list_models(&self) -> ProviderResult<Vec<String>> {
        let value = get_json(&format!("{}/api/tags", self.base_url), None, self.http)?;
        let models = value
            .get("models")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| {
                ProviderError::new(
                    ProviderErrorKind::InvalidJson,
                    "Ollama models must be an array",
                )
            })?;

        Ok(models
            .iter()
            .filter_map(|model| {
                model
                    .get("name")
                    .or_else(|| model.get("model"))
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .collect())
    }
}

impl ModelProvider for OllamaProvider {
    fn complete<'a>(&'a self, request: ModelRequest) -> ProviderFuture<'a, ModelResponse> {
        Box::pin(async move {
            let body = serde_json::json!({
                "model": request.model,
                "system": request.system.unwrap_or_default(),
                "prompt": request.input,
                "stream": false,
                "keep_alive": self.keep_alive,
                "options": {
                    "temperature": request.temperature.unwrap_or(self.temperature),
                    "seed": self.seed,
                    "num_ctx": self.num_ctx,
                    "num_predict": request.max_output_tokens.unwrap_or(self.num_predict),
                }
            });
            let value = post_json(
                &format!("{}/api/generate", self.base_url),
                body,
                None,
                self.http,
            )?;
            parse_ollama_response(value)
        })
    }
}

#[derive(Debug, Clone)]
pub struct OpenAiCompatibleProvider {
    base_url: String,
    api_key: String,
    max_output_tokens: u32,
    http: ProviderHttpSettings,
}

impl OpenAiCompatibleProvider {
    pub fn new(config: &OpenAiConfig, api_key: impl Into<String>) -> ProviderResult<Self> {
        DefaultProviderUrlPolicy::new()
            .validate_credentialed_url(&config.base_url, config.allow_untrusted_provider_url)
            .map_err(|err| {
                ProviderError::new(
                    ProviderErrorKind::BadRequest,
                    format!("provider URL rejected: {err}"),
                )
            })?;

        Ok(Self {
            base_url: trim_base_url(&config.base_url),
            api_key: api_key.into(),
            max_output_tokens: config.max_output_tokens,
            http: ProviderHttpSettings::new(
                config.connect_timeout_seconds,
                config.timeout_seconds,
                config.max_retries,
                config.retry_backoff_ms,
            ),
        })
    }

    pub fn from_env(config: &OpenAiConfig) -> ProviderResult<Self> {
        let api_key = std::env::var(&config.api_key_env)
            .or_else(|_| std::env::var(&config.fallback_api_key_env))
            .map_err(|_| {
                ProviderError::new(
                    ProviderErrorKind::AuthFailed,
                    format!(
                        "missing API key env var {} or {}",
                        config.api_key_env, config.fallback_api_key_env
                    ),
                )
            })?;
        Self::new(config, api_key)
    }

    pub fn list_models(&self) -> ProviderResult<Vec<String>> {
        let value = get_json(
            &format!("{}/models", self.base_url),
            Some(self.api_key.as_str()),
            self.http,
        )?;
        let models = value
            .get("data")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| {
                ProviderError::new(
                    ProviderErrorKind::InvalidJson,
                    "OpenAI models data must be an array",
                )
            })?;

        Ok(models
            .iter()
            .filter_map(|model| {
                model
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .collect())
    }
}

impl ModelProvider for OpenAiCompatibleProvider {
    fn complete<'a>(&'a self, request: ModelRequest) -> ProviderFuture<'a, ModelResponse> {
        Box::pin(async move {
            let max_output_tokens = request.max_output_tokens.unwrap_or(self.max_output_tokens);
            let body = serde_json::json!({
                "model": request.model,
                "instructions": request.system.unwrap_or_default(),
                "input": request.input,
                "stream": false,
                "store": false,
                "max_output_tokens": max_output_tokens,
                "metadata": request.metadata,
            });
            let value = post_json(
                &format!("{}/responses", self.base_url),
                body,
                Some(self.api_key.as_str()),
                self.http,
            )?;
            parse_openai_response(value)
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FakeProviderStep {
    Text {
        response_id: Option<String>,
        text: String,
    },
    Response(ModelResponse),
    Error(ProviderError),
}

#[derive(Debug, Clone)]
pub struct FakeModelProvider {
    provider: String,
    steps: Arc<Mutex<VecDeque<FakeProviderStep>>>,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
}

impl FakeModelProvider {
    pub fn new(provider: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            steps: Arc::new(Mutex::new(VecDeque::new())),
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn push_text(&self, text: impl Into<String>) {
        self.steps
            .lock()
            .expect("fake provider step lock poisoned")
            .push_back(FakeProviderStep::Text {
                response_id: None,
                text: text.into(),
            });
    }

    pub fn push_text_with_id(&self, response_id: impl Into<String>, text: impl Into<String>) {
        self.steps
            .lock()
            .expect("fake provider step lock poisoned")
            .push_back(FakeProviderStep::Text {
                response_id: Some(response_id.into()),
                text: text.into(),
            });
    }

    pub fn push_response(&self, response: ModelResponse) {
        self.steps
            .lock()
            .expect("fake provider step lock poisoned")
            .push_back(FakeProviderStep::Response(response));
    }

    pub fn push_error(&self, kind: ProviderErrorKind, message: impl Into<String>) {
        self.steps
            .lock()
            .expect("fake provider step lock poisoned")
            .push_back(FakeProviderStep::Error(ProviderError::new(kind, message)));
    }

    pub fn requests(&self) -> Vec<ModelRequest> {
        self.requests
            .lock()
            .expect("fake provider request lock poisoned")
            .clone()
    }

    pub fn pending_steps(&self) -> usize {
        self.steps
            .lock()
            .expect("fake provider step lock poisoned")
            .len()
    }
}

fn parse_ollama_response(value: serde_json::Value) -> ProviderResult<ModelResponse> {
    if value.get("done").and_then(serde_json::Value::as_bool) != Some(true) {
        return Err(ProviderError::new(
            ProviderErrorKind::IncompleteResponse,
            "Ollama response did not report done=true",
        ));
    }
    let model = required_string(&value, "model", ProviderErrorKind::InvalidJson)?;
    let text = required_string(&value, "response", ProviderErrorKind::EmptyOutput)?;
    if text.is_empty() {
        return Err(ProviderError::new(
            ProviderErrorKind::EmptyOutput,
            "Ollama response text was empty",
        ));
    }
    Ok(ModelResponse {
        provider: "ollama".to_string(),
        model,
        response_id: None,
        text,
    })
}

fn parse_openai_response(value: serde_json::Value) -> ProviderResult<ModelResponse> {
    if let Some(error) = value.get("error") {
        return Err(ProviderError::new(
            ProviderErrorKind::BadRequest,
            format!("OpenAI response error: {error}"),
        ));
    }
    if value.get("status").and_then(serde_json::Value::as_str) != Some("completed") {
        return Err(ProviderError::new(
            ProviderErrorKind::IncompleteResponse,
            "OpenAI response status was not completed",
        ));
    }
    if let Some(incomplete) = value.get("incomplete_details") {
        return Err(ProviderError::new(
            ProviderErrorKind::IncompleteResponse,
            format!("OpenAI response was incomplete: {incomplete}"),
        ));
    }

    let response_id = value
        .get("id")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned);
    let model = required_string(&value, "model", ProviderErrorKind::InvalidJson)?;
    let mut output = String::new();

    for item in value
        .get("output")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            ProviderError::new(
                ProviderErrorKind::InvalidJson,
                "OpenAI output must be an array",
            )
        })?
    {
        if let Some(content) = item.get("content").and_then(serde_json::Value::as_array) {
            for part in content {
                if part.get("type").and_then(serde_json::Value::as_str) == Some("output_text") {
                    if let Some(text) = part.get("text").and_then(serde_json::Value::as_str) {
                        output.push_str(text);
                    }
                }
            }
        }
    }

    if output.is_empty() {
        return Err(ProviderError::new(
            ProviderErrorKind::EmptyOutput,
            "OpenAI response contained no output_text",
        ));
    }

    Ok(ModelResponse {
        provider: "openai-compatible".to_string(),
        model,
        response_id,
        text: output,
    })
}

fn required_string(
    value: &serde_json::Value,
    field: &str,
    empty_kind: ProviderErrorKind,
) -> ProviderResult<String> {
    let text = value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            ProviderError::new(
                ProviderErrorKind::InvalidJson,
                format!("missing string field {field}"),
            )
        })?
        .to_string();
    if text.is_empty() {
        Err(ProviderError::new(
            empty_kind,
            format!("field {field} was empty"),
        ))
    } else {
        Ok(text)
    }
}

fn get_json(
    url: &str,
    api_key: Option<&str>,
    settings: ProviderHttpSettings,
) -> ProviderResult<serde_json::Value> {
    with_retries(settings.retry_policy, || {
        let agent = settings.agent();
        let mut request = agent.get(url);
        if let Some(api_key) = api_key {
            request = request.set("Authorization", &format!("Bearer {api_key}"));
        }
        request
            .call()
            .map_err(map_ureq_error)?
            .into_json()
            .map_err(|err| ProviderError::new(ProviderErrorKind::InvalidJson, err.to_string()))
    })
}

fn post_json(
    url: &str,
    body: serde_json::Value,
    api_key: Option<&str>,
    settings: ProviderHttpSettings,
) -> ProviderResult<serde_json::Value> {
    with_retries(settings.retry_policy, || {
        let agent = settings.agent();
        let mut request = agent.post(url);
        if let Some(api_key) = api_key {
            request = request.set("Authorization", &format!("Bearer {api_key}"));
        }
        request
            .send_json(body.clone())
            .map_err(map_ureq_error)?
            .into_json()
            .map_err(|err| ProviderError::new(ProviderErrorKind::InvalidJson, err.to_string()))
    })
}

fn with_retries<T>(
    policy: RetryPolicy,
    mut operation: impl FnMut() -> ProviderResult<T>,
) -> ProviderResult<T> {
    let mut completed_retries = 0;
    loop {
        match operation() {
            Ok(value) => return Ok(value),
            Err(error) if policy.should_retry(&error, completed_retries) => {
                completed_retries += 1;
                if !policy.backoff.is_zero() {
                    thread::sleep(policy.backoff);
                }
            }
            Err(error) => return Err(error),
        }
    }
}

fn map_ureq_error(error: ureq::Error) -> ProviderError {
    match error {
        ureq::Error::Status(status, response) => {
            let body = response.into_string().unwrap_or_default();
            let redacted = DefaultRedactor::new().redact(&body).text;
            let kind = match status {
                400..=499 if status == 401 || status == 403 => ProviderErrorKind::AuthFailed,
                404 => ProviderErrorKind::ModelMissing,
                429 => ProviderErrorKind::RateLimited,
                400..=499 => ProviderErrorKind::BadRequest,
                500..=599 => ProviderErrorKind::HttpServer,
                _ => ProviderErrorKind::BadRequest,
            };
            ProviderError::new(kind, format!("HTTP {status}: {redacted}"))
        }
        ureq::Error::Transport(err) => {
            let message = err.to_string();
            let kind = if message.to_ascii_lowercase().contains("timed out") {
                ProviderErrorKind::Timeout
            } else {
                ProviderErrorKind::HttpServer
            };
            ProviderError::new(kind, message)
        }
    }
}

fn trim_base_url(value: &str) -> String {
    value.trim().trim_end_matches('/').to_string()
}

impl ModelProvider for FakeModelProvider {
    fn complete<'a>(&'a self, request: ModelRequest) -> ProviderFuture<'a, ModelResponse> {
        Box::pin(async move {
            self.requests
                .lock()
                .expect("fake provider request lock poisoned")
                .push(request.clone());

            match self
                .steps
                .lock()
                .expect("fake provider step lock poisoned")
                .pop_front()
            {
                Some(FakeProviderStep::Text { response_id, text }) => Ok(ModelResponse {
                    provider: self.provider.clone(),
                    model: request.model,
                    response_id,
                    text,
                }),
                Some(FakeProviderStep::Response(response)) => Ok(response),
                Some(FakeProviderStep::Error(error)) => Err(error),
                None => Err(ProviderError::new(
                    ProviderErrorKind::BadRequest,
                    "fake provider has no queued response",
                )),
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeHttpRequest {
    pub method: String,
    pub path: String,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl FakeHttpRequest {
    pub fn body_string(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn json_body(&self) -> serde_json::Result<serde_json::Value> {
        serde_json::from_slice(&self.body)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeHttpResponse {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
    pub delay: Option<Duration>,
    pub drop_connection: bool,
}

impl FakeHttpResponse {
    pub fn new(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            headers: BTreeMap::new(),
            body: body.into(),
            delay: None,
            drop_connection: false,
        }
    }

    pub fn json(status: u16, value: serde_json::Value) -> Self {
        let mut response = Self::new(status, value.to_string());
        response
            .headers
            .insert("content-type".to_string(), "application/json".to_string());
        response
    }

    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(name.into(), value.into());
        self
    }

    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = Some(delay);
        self
    }

    pub fn drop_connection() -> Self {
        Self {
            status: 0,
            headers: BTreeMap::new(),
            body: Vec::new(),
            delay: None,
            drop_connection: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FakeHttpExpectation {
    OllamaGenerate { model: String },
    OpenAiResponses { model: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeHttpRoute {
    pub method: String,
    pub path: String,
    pub responses: Vec<FakeHttpResponse>,
    pub repeat_last: bool,
    pub expectation: Option<FakeHttpExpectation>,
}

impl FakeHttpRoute {
    pub fn new(
        method: impl Into<String>,
        path: impl Into<String>,
        response: FakeHttpResponse,
    ) -> Self {
        Self {
            method: method.into(),
            path: path.into(),
            responses: vec![response],
            repeat_last: true,
            expectation: None,
        }
    }

    pub fn sequence(
        method: impl Into<String>,
        path: impl Into<String>,
        responses: Vec<FakeHttpResponse>,
    ) -> Self {
        assert!(
            !responses.is_empty(),
            "fake HTTP route sequences require at least one response"
        );
        Self {
            method: method.into(),
            path: path.into(),
            responses,
            repeat_last: false,
            expectation: None,
        }
    }

    pub fn with_expectation(mut self, expectation: FakeHttpExpectation) -> Self {
        self.expectation = Some(expectation);
        self
    }
}

#[derive(Debug)]
struct FakeHttpServerState {
    requests: Mutex<Vec<FakeHttpRequest>>,
    routes: Mutex<Vec<FakeHttpRoute>>,
    shutdown: AtomicBool,
}

#[derive(Debug)]
pub struct FakeHttpServer {
    addr: SocketAddr,
    state: Arc<FakeHttpServerState>,
    worker: Option<JoinHandle<()>>,
}

impl FakeHttpServer {
    pub fn start(routes: Vec<FakeHttpRoute>) -> io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let addr = listener.local_addr()?;
        let state = Arc::new(FakeHttpServerState {
            requests: Mutex::new(Vec::new()),
            routes: Mutex::new(routes),
            shutdown: AtomicBool::new(false),
        });
        let worker_state = Arc::clone(&state);
        let worker = thread::spawn(move || {
            for stream in listener.incoming() {
                if worker_state.shutdown.load(Ordering::SeqCst) {
                    break;
                }

                if let Ok(mut stream) = stream {
                    let _ = handle_fake_http_connection(&worker_state, &mut stream);
                }
            }
        });

        Ok(Self {
            addr,
            state,
            worker: Some(worker),
        })
    }

    pub fn ollama_success(model: &str, response_text: &str) -> io::Result<Self> {
        Self::start(vec![
            FakeHttpRoute::new(
                "GET",
                "/api/tags",
                FakeHttpResponse::json(
                    200,
                    serde_json::json!({
                        "models": [
                            {
                                "name": model,
                                "model": model,
                            }
                        ]
                    }),
                ),
            ),
            FakeHttpRoute::new(
                "POST",
                "/api/generate",
                FakeHttpResponse::json(
                    200,
                    serde_json::json!({
                        "model": model,
                        "response": response_text,
                        "done": true,
                    }),
                ),
            )
            .with_expectation(FakeHttpExpectation::OllamaGenerate {
                model: model.to_string(),
            }),
        ])
    }

    pub fn openai_success(model: &str, response_id: &str, output_text: &str) -> io::Result<Self> {
        Self::start(vec![
            FakeHttpRoute::new(
                "GET",
                "/models",
                FakeHttpResponse::json(
                    200,
                    serde_json::json!({
                        "data": [
                            {
                                "id": model,
                            }
                        ]
                    }),
                ),
            ),
            FakeHttpRoute::new(
                "POST",
                "/responses",
                FakeHttpResponse::json(
                    200,
                    serde_json::json!({
                        "model": model,
                        "id": response_id,
                        "status": "completed",
                        "output": [
                            {
                                "type": "message",
                                "content": [
                                    {
                                        "type": "output_text",
                                        "text": output_text,
                                    }
                                ]
                            }
                        ],
                    }),
                ),
            )
            .with_expectation(FakeHttpExpectation::OpenAiResponses {
                model: model.to_string(),
            }),
        ])
    }

    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn requests(&self) -> Vec<FakeHttpRequest> {
        self.state
            .requests
            .lock()
            .expect("fake http request lock poisoned")
            .clone()
    }
}

impl Drop for FakeHttpServer {
    fn drop(&mut self) {
        self.state.shutdown.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn handle_fake_http_connection(
    state: &FakeHttpServerState,
    stream: &mut TcpStream,
) -> io::Result<()> {
    let request = read_fake_http_request(stream)?;
    let response = next_fake_http_response(state, &request);

    state
        .requests
        .lock()
        .expect("fake http request lock poisoned")
        .push(request);

    if let Some(delay) = response.delay {
        thread::sleep(delay);
    }
    if response.drop_connection {
        return Ok(());
    }

    write_fake_http_response(stream, response)
}

fn next_fake_http_response(
    state: &FakeHttpServerState,
    request: &FakeHttpRequest,
) -> FakeHttpResponse {
    let mut routes = state.routes.lock().expect("fake http route lock poisoned");
    if let Some(route) = routes
        .iter_mut()
        .find(|route| route.method == request.method && route.path == request.path)
    {
        if let Some(error) = validate_fake_http_expectation(route, request) {
            return FakeHttpResponse::json(400, serde_json::json!({ "error": error }));
        }
        if route.repeat_last {
            route.responses[0].clone()
        } else if !route.responses.is_empty() {
            route.responses.remove(0)
        } else {
            FakeHttpResponse::json(500, serde_json::json!({ "error": "fake route exhausted" }))
        }
    } else {
        FakeHttpResponse::new(404, "not found")
    }
}

fn validate_fake_http_expectation(
    route: &FakeHttpRoute,
    request: &FakeHttpRequest,
) -> Option<String> {
    let expectation = route.expectation.as_ref()?;
    let body = match request.json_body() {
        Ok(body) => body,
        Err(err) => return Some(format!("request body must be JSON: {err}")),
    };

    match expectation {
        FakeHttpExpectation::OllamaGenerate { model } => {
            if let Err(error) = require_json_eq(&body, "model", serde_json::json!(model)) {
                return Some(error);
            }
            if let Err(error) = require_json_eq(&body, "stream", serde_json::json!(false)) {
                return Some(error);
            }
            if let Err(error) = require_json_string(&body, "prompt") {
                return Some(error);
            }
            if let Err(error) = require_json_string(&body, "system") {
                return Some(error);
            }
            if let Err(error) = require_json_string(&body, "keep_alive") {
                return Some(error);
            }
            let Some(options) = body.get("options") else {
                return Some("options is required".to_string());
            };
            if !options.is_object() {
                return Some("options must be an object".to_string());
            }
            for field in ["temperature", "seed", "num_ctx", "num_predict"] {
                if let Err(error) = require_json_number(options, field) {
                    return Some(format!("options.{error}"));
                }
            }
            None
        }
        FakeHttpExpectation::OpenAiResponses { model } => {
            if !request.headers.contains_key("authorization") {
                return Some("authorization header is required".to_string());
            }
            if let Err(error) = require_json_eq(&body, "model", serde_json::json!(model)) {
                return Some(error);
            }
            if let Err(error) = require_json_eq(&body, "stream", serde_json::json!(false)) {
                return Some(error);
            }
            if let Err(error) = require_json_eq(&body, "store", serde_json::json!(false)) {
                return Some(error);
            }
            if let Err(error) = require_json_string(&body, "input") {
                return Some(error);
            }
            if let Err(error) = require_json_string(&body, "instructions") {
                return Some(error);
            }
            if !body
                .get("metadata")
                .is_some_and(serde_json::Value::is_object)
            {
                return Some("metadata object is required".to_string());
            }
            if !body
                .get("max_output_tokens")
                .is_some_and(serde_json::Value::is_number)
            {
                return Some("max_output_tokens number is required".to_string());
            }
            None
        }
    }
}

fn require_json_eq(
    body: &serde_json::Value,
    field: &str,
    expected: serde_json::Value,
) -> Result<(), String> {
    match body.get(field) {
        Some(actual) if actual == &expected => Ok(()),
        Some(actual) => Err(format!("{field} must be {expected}, got {actual}")),
        None => Err(format!("{field} is required")),
    }
}

fn require_json_string(body: &serde_json::Value, field: &str) -> Result<(), String> {
    match body.get(field) {
        Some(value) if value.is_string() => Ok(()),
        Some(_) => Err(format!("{field} must be a string")),
        None => Err(format!("{field} is required")),
    }
}

fn require_json_number(body: &serde_json::Value, field: &str) -> Result<(), String> {
    match body.get(field) {
        Some(value) if value.is_number() => Ok(()),
        Some(_) => Err(format!("{field} must be a number")),
        None => Err(format!("{field} is required")),
    }
}

fn read_fake_http_request(stream: &mut TcpStream) -> io::Result<FakeHttpRequest> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 1024];
    let header_end = loop {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "connection closed before headers",
            ));
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(index) = find_header_end(&buffer) {
            break index;
        }
    };

    let headers_bytes = &buffer[..header_end];
    let headers_text = String::from_utf8_lossy(headers_bytes);
    let mut lines = headers_text.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts.next().unwrap_or_default().to_string();
    let path = request_parts.next().unwrap_or_default().to_string();
    let mut headers = BTreeMap::new();

    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }

    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let body_start = header_end + 4;
    while buffer.len().saturating_sub(body_start) < content_length {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "connection closed before body",
            ));
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    let body = buffer[body_start..body_start + content_length].to_vec();

    Ok(FakeHttpRequest {
        method,
        path,
        headers,
        body,
    })
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn write_fake_http_response(
    stream: &mut TcpStream,
    mut response: FakeHttpResponse,
) -> io::Result<()> {
    response
        .headers
        .entry("content-length".to_string())
        .or_insert_with(|| response.body.len().to_string());
    response
        .headers
        .entry("connection".to_string())
        .or_insert_with(|| "close".to_string());

    write!(stream, "HTTP/1.1 {} OK\r\n", response.status)?;
    for (name, value) in response.headers {
        write!(stream, "{name}: {value}\r\n")?;
    }
    stream.write_all(b"\r\n")?;
    stream.write_all(&response.body)?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block_on_provider<T>(future: ProviderFuture<'_, T>) -> ProviderResult<T> {
        let waker = std::task::Waker::noop();
        let mut context = std::task::Context::from_waker(waker);
        let mut future = future;
        loop {
            match future.as_mut().poll(&mut context) {
                std::task::Poll::Ready(value) => return value,
                std::task::Poll::Pending => thread::yield_now(),
            }
        }
    }

    fn model_request(input: &str) -> ModelRequest {
        ModelRequest {
            model: "test-model".to_string(),
            system: Some("system".to_string()),
            input: input.to_string(),
            temperature: Some(0.0),
            max_output_tokens: Some(128),
            metadata: BTreeMap::from([("task_id".to_string(), "task_1".to_string())]),
        }
    }

    #[test]
    fn fake_provider_returns_queued_text_and_records_requests() {
        let provider = FakeModelProvider::new("fake-local");
        provider.push_text_with_id("resp_1", "patched");

        let response = block_on_provider(provider.complete(model_request("fix it"))).unwrap();

        assert_eq!(response.provider, "fake-local");
        assert_eq!(response.model, "test-model");
        assert_eq!(response.response_id.as_deref(), Some("resp_1"));
        assert_eq!(response.text, "patched");
        assert_eq!(provider.pending_steps(), 0);
        assert_eq!(provider.requests(), vec![model_request("fix it")]);
    }

    #[test]
    fn fake_provider_returns_queued_errors_deterministically() {
        let provider = FakeModelProvider::new("fake-local");
        provider.push_error(ProviderErrorKind::RateLimited, "retry later");

        let error = block_on_provider(provider.complete(model_request("fix it"))).unwrap_err();

        assert_eq!(error.kind, ProviderErrorKind::RateLimited);
        assert_eq!(error.message, "retry later");
        assert!(error.is_retryable());
    }

    #[test]
    fn retry_policy_retries_only_retryable_errors_within_budget() {
        let policy = RetryPolicy::new(2, Duration::from_millis(10));
        let rate_limited = ProviderError::new(ProviderErrorKind::RateLimited, "limited");
        let bad_request = ProviderError::new(ProviderErrorKind::BadRequest, "bad");

        assert_eq!(policy.max_attempts(), 3);
        assert!(policy.should_retry(&rate_limited, 0));
        assert!(policy.should_retry(&rate_limited, 1));
        assert!(!policy.should_retry(&rate_limited, 2));
        assert!(!policy.should_retry(&bad_request, 0));
    }

    #[test]
    fn fake_http_server_serves_routes_and_records_requests() {
        let server = FakeHttpServer::start(vec![FakeHttpRoute::new(
            "POST",
            "/responses",
            FakeHttpResponse::json(200, serde_json::json!({"ok": true})),
        )])
        .unwrap();

        let response: serde_json::Value = ureq::post(&format!("{}/responses", server.base_url()))
            .set("Authorization", "Bearer test")
            .send_json(serde_json::json!({"input": "hello"}))
            .unwrap()
            .into_json()
            .unwrap();

        assert_eq!(response, serde_json::json!({"ok": true}));
        let requests = server.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "POST");
        assert_eq!(requests[0].path, "/responses");
        assert_eq!(
            requests[0].headers.get("authorization").map(String::as_str),
            Some("Bearer test")
        );
        assert_eq!(
            requests[0].json_body().unwrap(),
            serde_json::json!({"input": "hello"})
        );
    }

    #[test]
    fn fake_http_server_can_sequence_retry_responses() {
        let server = FakeHttpServer::start(vec![FakeHttpRoute::sequence(
            "POST",
            "/responses",
            vec![
                FakeHttpResponse::json(500, serde_json::json!({"error": "temporary"})),
                FakeHttpResponse::json(200, serde_json::json!({"ok": true})),
            ],
        )])
        .unwrap();

        let first = ureq::post(&format!("{}/responses", server.base_url()))
            .send_json(serde_json::json!({"try": 1}))
            .unwrap_err();
        assert_eq!(first.into_response().unwrap().status(), 500);

        let second: serde_json::Value = ureq::post(&format!("{}/responses", server.base_url()))
            .send_json(serde_json::json!({"try": 2}))
            .unwrap()
            .into_json()
            .unwrap();
        assert_eq!(second, serde_json::json!({"ok": true}));
        let exhausted = ureq::post(&format!("{}/responses", server.base_url()))
            .send_json(serde_json::json!({"try": 3}))
            .unwrap_err();
        assert_eq!(exhausted.into_response().unwrap().status(), 500);
        assert_eq!(server.requests().len(), 3);
    }

    #[test]
    fn fake_http_server_has_ollama_and_openai_success_helpers() {
        let ollama = FakeHttpServer::ollama_success("local-model", "done").unwrap();
        let ollama_tags: serde_json::Value = ureq::get(&format!("{}/api/tags", ollama.base_url()))
            .call()
            .unwrap()
            .into_json()
            .unwrap();
        assert_eq!(ollama_tags["models"][0]["name"], "local-model");
        let ollama_generate: serde_json::Value =
            ureq::post(&format!("{}/api/generate", ollama.base_url()))
                .send_json(serde_json::json!({
                    "model": "local-model",
                    "system": "follow the contract",
                    "prompt": "fix",
                    "stream": false,
                    "keep_alive": "5m",
                    "options": {
                        "temperature": 0.0,
                        "seed": 42,
                        "num_ctx": 8192,
                        "num_predict": 2048
                    }
                }))
                .unwrap()
                .into_json()
                .unwrap();
        assert_eq!(ollama_generate["response"], "done");
        let bad_ollama = ureq::post(&format!("{}/api/generate", ollama.base_url()))
            .send_json(serde_json::json!({
                "model": "wrong",
                "system": "follow the contract",
                "prompt": "fix",
                "stream": false,
                "keep_alive": "5m",
                "options": {}
            }))
            .unwrap_err();
        assert_eq!(bad_ollama.into_response().unwrap().status(), 400);

        let openai = FakeHttpServer::openai_success("gpt-test", "resp_1", "answer").unwrap();
        let openai_models: serde_json::Value = ureq::get(&format!("{}/models", openai.base_url()))
            .call()
            .unwrap()
            .into_json()
            .unwrap();
        assert_eq!(openai_models["data"][0]["id"], "gpt-test");
        let openai_response: serde_json::Value =
            ureq::post(&format!("{}/responses", openai.base_url()))
                .set("Authorization", "Bearer test")
                .send_json(serde_json::json!({
                    "model": "gpt-test",
                    "instructions": "help",
                    "input": "question",
                    "stream": false,
                    "store": false,
                    "max_output_tokens": 4096,
                    "metadata": {"ticket_id": "ticket_01ARZ3NDEKTSV4RRFFQ69G5FAV"}
                }))
                .unwrap()
                .into_json()
                .unwrap();
        assert_eq!(openai_response["id"], "resp_1");
        let bad_openai = ureq::post(&format!("{}/responses", openai.base_url()))
            .send_json(serde_json::json!({
                "model": "gpt-test",
                "instructions": "help",
                "input": "question",
                "stream": false,
                "store": false,
                "max_output_tokens": 4096,
                "metadata": {}
            }))
            .unwrap_err();
        assert_eq!(bad_openai.into_response().unwrap().status(), 400);
    }

    #[test]
    fn ollama_provider_lists_models_and_generates() {
        let server = FakeHttpServer::ollama_success("local-model", "patched").unwrap();
        let mut config = crate::domain::HarnessConfig::default().providers.ollama;
        config.base_url = server.base_url();
        config.default_model = "local-model".to_string();
        let provider = OllamaProvider::new(&config);

        assert_eq!(provider.list_models().unwrap(), vec!["local-model"]);
        let response = block_on_provider(provider.complete(ModelRequest {
            model: "local-model".to_string(),
            system: Some("system".to_string()),
            input: "fix".to_string(),
            temperature: Some(0.0),
            max_output_tokens: Some(64),
            metadata: BTreeMap::new(),
        }))
        .unwrap();

        assert_eq!(response.provider, "ollama");
        assert_eq!(response.model, "local-model");
        assert_eq!(response.text, "patched");
        let requests = server.requests();
        let generate = requests
            .iter()
            .find(|request| request.path == "/api/generate")
            .unwrap()
            .json_body()
            .unwrap();
        assert_eq!(generate["system"], "system");
        assert_eq!(generate["options"]["num_predict"], 64);
    }

    #[test]
    fn openai_provider_lists_models_and_extracts_output_text() {
        let server = FakeHttpServer::openai_success("gpt-test", "resp_1", "answer").unwrap();
        let mut config = crate::domain::HarnessConfig::default().providers.openai;
        config.base_url = server.base_url();
        config.default_model = "gpt-test".to_string();
        config.allow_untrusted_provider_url = true;
        let provider = OpenAiCompatibleProvider::new(&config, "test-key").unwrap();

        assert_eq!(provider.list_models().unwrap(), vec!["gpt-test"]);
        let response = block_on_provider(provider.complete(ModelRequest {
            model: "gpt-test".to_string(),
            system: Some("instructions".to_string()),
            input: "question".to_string(),
            temperature: None,
            max_output_tokens: Some(256),
            metadata: BTreeMap::from([(
                "ticket_id".to_string(),
                "ticket_01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string(),
            )]),
        }))
        .unwrap();

        assert_eq!(response.provider, "openai-compatible");
        assert_eq!(response.model, "gpt-test");
        assert_eq!(response.response_id.as_deref(), Some("resp_1"));
        assert_eq!(response.text, "answer");
        let requests = server.requests();
        let responses = requests
            .iter()
            .find(|request| request.path == "/responses")
            .unwrap();
        assert_eq!(
            responses.headers.get("authorization").map(String::as_str),
            Some("Bearer test-key")
        );
        assert_eq!(responses.json_body().unwrap()["store"], false);
    }

    #[test]
    fn openai_parser_rejects_incomplete_and_empty_output() {
        let incomplete = parse_openai_response(serde_json::json!({
            "id": "resp_1",
            "model": "gpt-test",
            "status": "incomplete",
            "output": []
        }))
        .unwrap_err();
        assert_eq!(incomplete.kind, ProviderErrorKind::IncompleteResponse);

        let empty = parse_openai_response(serde_json::json!({
            "id": "resp_1",
            "model": "gpt-test",
            "status": "completed",
            "output": [{"type": "message", "content": []}]
        }))
        .unwrap_err();
        assert_eq!(empty.kind, ProviderErrorKind::EmptyOutput);
    }

    #[test]
    fn openai_provider_rejects_untrusted_credentialed_base_url() {
        let mut config = crate::domain::HarnessConfig::default().providers.openai;
        config.base_url = "http://example.test/v1".to_string();
        config.allow_untrusted_provider_url = false;

        let error = OpenAiCompatibleProvider::new(&config, "test-key").unwrap_err();

        assert_eq!(error.kind, ProviderErrorKind::BadRequest);
        assert!(error.message.contains("provider URL rejected"));
    }

    #[test]
    fn openai_parser_rejects_missing_and_non_string_model() {
        let missing = parse_openai_response(serde_json::json!({
            "id": "resp_1",
            "status": "completed",
            "output": [{"content": [{"type": "output_text", "text": "answer"}]}]
        }))
        .unwrap_err();
        assert_eq!(missing.kind, ProviderErrorKind::InvalidJson);

        let non_string = parse_openai_response(serde_json::json!({
            "id": "resp_1",
            "model": 123,
            "status": "completed",
            "output": [{"content": [{"type": "output_text", "text": "answer"}]}]
        }))
        .unwrap_err();
        assert_eq!(non_string.kind, ProviderErrorKind::InvalidJson);
    }

    #[test]
    fn provider_http_error_bodies_are_redacted() {
        let server = FakeHttpServer::start(vec![FakeHttpRoute::new(
            "GET",
            "/models",
            FakeHttpResponse::new(401, "api_key = sk-test_1234567890abcdefghijklmnop"),
        )])
        .unwrap();
        let mut config = crate::domain::HarnessConfig::default().providers.openai;
        config.base_url = server.base_url();
        config.allow_untrusted_provider_url = true;
        let provider = OpenAiCompatibleProvider::new(&config, "test-key").unwrap();

        let error = provider.list_models().unwrap_err();

        assert_eq!(error.kind, ProviderErrorKind::AuthFailed);
        assert!(error.message.contains("[REDACTED"));
        assert!(!error.message.contains("sk-test"));
    }

    #[test]
    fn provider_maps_429_from_fake_http_server() {
        let server = FakeHttpServer::start(vec![FakeHttpRoute::new(
            "GET",
            "/models",
            FakeHttpResponse::json(429, serde_json::json!({"error": "limited"})),
        )])
        .unwrap();
        let mut config = crate::domain::HarnessConfig::default().providers.openai;
        config.base_url = server.base_url();
        config.allow_untrusted_provider_url = true;
        config.max_retries = 0;
        let provider = OpenAiCompatibleProvider::new(&config, "test-key").unwrap();

        let error = provider.list_models().unwrap_err();

        assert_eq!(error.kind, ProviderErrorKind::RateLimited);
        assert!(error.is_retryable());
    }

    #[test]
    fn fake_http_provider_surfaces_malformed_json_and_missing_model() {
        let malformed = FakeHttpServer::start(vec![FakeHttpRoute::new(
            "GET",
            "/models",
            FakeHttpResponse::new(200, "{not json"),
        )])
        .unwrap();
        let mut config = crate::domain::HarnessConfig::default().providers.openai;
        config.base_url = malformed.base_url();
        config.allow_untrusted_provider_url = true;
        let provider = OpenAiCompatibleProvider::new(&config, "test-key").unwrap();

        let error = provider.list_models().unwrap_err();
        assert_eq!(error.kind, ProviderErrorKind::InvalidJson);

        let missing_model = FakeHttpServer::start(vec![
            FakeHttpRoute::new(
                "POST",
                "/responses",
                FakeHttpResponse::json(
                    200,
                    serde_json::json!({
                        "id": "resp_1",
                        "status": "completed",
                        "output": [{"content": [{"type": "output_text", "text": "answer"}]}]
                    }),
                ),
            )
            .with_expectation(FakeHttpExpectation::OpenAiResponses {
                model: "gpt-test".to_string(),
            }),
        ])
        .unwrap();
        config.base_url = missing_model.base_url();
        let provider = OpenAiCompatibleProvider::new(&config, "test-key").unwrap();

        let error = block_on_provider(provider.complete(ModelRequest {
            model: "gpt-test".to_string(),
            system: Some("instructions".to_string()),
            input: "question".to_string(),
            temperature: None,
            max_output_tokens: Some(256),
            metadata: BTreeMap::new(),
        }))
        .unwrap_err();
        assert_eq!(error.kind, ProviderErrorKind::InvalidJson);
    }

    #[test]
    fn openai_provider_uses_configured_retry_policy() {
        let server = FakeHttpServer::start(vec![
            FakeHttpRoute::sequence(
                "POST",
                "/responses",
                vec![
                    FakeHttpResponse::json(500, serde_json::json!({"error": "temporary"})),
                    FakeHttpResponse::json(
                        200,
                        serde_json::json!({
                            "model": "gpt-test",
                            "id": "resp_1",
                            "status": "completed",
                            "output": [
                                {
                                    "content": [
                                        {"type": "output_text", "text": "answer"}
                                    ]
                                }
                            ],
                        }),
                    ),
                ],
            )
            .with_expectation(FakeHttpExpectation::OpenAiResponses {
                model: "gpt-test".to_string(),
            }),
        ])
        .unwrap();
        let mut config = crate::domain::HarnessConfig::default().providers.openai;
        config.base_url = server.base_url();
        config.allow_untrusted_provider_url = true;
        config.max_retries = 1;
        config.retry_backoff_ms = 0;
        let provider = OpenAiCompatibleProvider::new(&config, "test-key").unwrap();

        let response = block_on_provider(provider.complete(ModelRequest {
            model: "gpt-test".to_string(),
            system: Some("instructions".to_string()),
            input: "question".to_string(),
            temperature: None,
            max_output_tokens: Some(256),
            metadata: BTreeMap::new(),
        }))
        .unwrap();

        assert_eq!(response.text, "answer");
        assert_eq!(server.requests().len(), 2);
    }

    #[test]
    fn ollama_provider_uses_configured_timeout() {
        let server = FakeHttpServer::start(vec![FakeHttpRoute::new(
            "GET",
            "/api/tags",
            FakeHttpResponse::json(200, serde_json::json!({"models": []}))
                .with_delay(Duration::from_secs(2)),
        )])
        .unwrap();
        let mut config = crate::domain::HarnessConfig::default().providers.ollama;
        config.base_url = server.base_url();
        config.timeout_seconds = 1;
        config.max_retries = 0;
        let provider = OllamaProvider::new(&config);

        let error = provider.list_models().unwrap_err();

        assert_eq!(error.kind, ProviderErrorKind::Timeout);
    }

    #[test]
    fn ollama_provider_uses_configured_temperature_when_request_omits_it() {
        let server = FakeHttpServer::ollama_success("local-model", "patched").unwrap();
        let mut config = crate::domain::HarnessConfig::default().providers.ollama;
        config.base_url = server.base_url();
        config.temperature = 0.7;
        let provider = OllamaProvider::new(&config);

        block_on_provider(provider.complete(ModelRequest {
            model: "local-model".to_string(),
            system: Some("system".to_string()),
            input: "fix".to_string(),
            temperature: None,
            max_output_tokens: None,
            metadata: BTreeMap::new(),
        }))
        .unwrap();

        let generate = server
            .requests()
            .into_iter()
            .find(|request| request.path == "/api/generate")
            .unwrap()
            .json_body()
            .unwrap();
        let temperature = generate["options"]["temperature"].as_f64().unwrap();
        assert!((temperature - 0.7).abs() < 0.000_001);
    }

    #[test]
    fn fake_http_server_can_drop_connections_for_transport_simulation() {
        let server = FakeHttpServer::start(vec![FakeHttpRoute::new(
            "GET",
            "/models",
            FakeHttpResponse::drop_connection(),
        )])
        .unwrap();

        let error = ureq::get(&format!("{}/models", server.base_url()))
            .call()
            .unwrap_err();

        assert!(matches!(error, ureq::Error::Transport(_)));
        assert_eq!(server.requests().len(), 1);
    }
}
