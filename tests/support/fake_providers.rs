#![allow(dead_code)]

use harness::domain::{ObjectiveId, TaskId, TicketId};
use harness::providers::{
    FakeHttpExpectation, FakeHttpRequest, FakeHttpResponse, FakeHttpRoute, FakeHttpServer,
    ModelRequest, ModelResponse, ProviderError, ProviderErrorKind, ProviderFuture, ProviderResult,
};
use serde_json::json;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

pub type FakeObjectiveProviderFuture<'a, T> =
    Pin<Box<dyn Future<Output = ProviderResult<T>> + Send + 'a>>;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderCallLedger {
    planner: Vec<FakePlannerRequest>,
    resolver: Vec<FakeTicketResolverRequest>,
    local: Vec<ModelRequestSnapshot>,
}

impl ProviderCallLedger {
    pub fn planner_count(&self) -> usize {
        self.planner.len()
    }

    pub fn resolver_count(&self) -> usize {
        self.resolver.len()
    }

    pub fn local_count(&self) -> usize {
        self.local.len()
    }

    pub fn planner_calls(&self) -> &[FakePlannerRequest] {
        &self.planner
    }

    pub fn resolver_calls(&self) -> &[FakeTicketResolverRequest] {
        &self.resolver
    }

    pub fn local_calls(&self) -> &[ModelRequestSnapshot] {
        &self.local
    }

    pub fn record_planner(&mut self, request: FakePlannerRequest) {
        self.planner.push(request);
    }

    pub fn record_resolver(&mut self, request: FakeTicketResolverRequest) {
        self.resolver.push(request);
    }

    pub fn record_local(&mut self, request: ModelRequest) {
        self.local.push(ModelRequestSnapshot::from(request));
    }

    pub fn assert_counts(&self, planner: usize, resolver: usize, local: usize) {
        assert_eq!(self.planner_count(), planner, "planner call count");
        assert_eq!(self.resolver_count(), resolver, "resolver call count");
        assert_eq!(self.local_count(), local, "local worker call count");
    }
}

pub type SharedProviderCallLedger = Arc<Mutex<ProviderCallLedger>>;

pub fn shared_provider_call_ledger() -> SharedProviderCallLedger {
    Arc::new(Mutex::new(ProviderCallLedger::default()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakePlannerRequest {
    pub objective_id: ObjectiveId,
    pub prompt: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakePlannerRawResponse {
    pub response_id: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeTicketResolverRequest {
    pub objective_id: ObjectiveId,
    pub ticket_id: TicketId,
    pub prompt: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeTicketResolverRawResponse {
    pub response_id: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRequestSnapshot {
    pub model: String,
    pub system: Option<String>,
    pub input: String,
}

impl From<ModelRequest> for ModelRequestSnapshot {
    fn from(request: ModelRequest) -> Self {
        Self {
            model: request.model,
            system: request.system,
            input: request.input,
        }
    }
}

pub trait FakeRemotePlanningProvider: Send + Sync {
    fn plan_objective<'a>(
        &'a self,
        request: FakePlannerRequest,
    ) -> FakeObjectiveProviderFuture<'a, FakePlannerRawResponse>;
}

pub trait FakeRemoteTicketResolverProvider: Send + Sync {
    fn resolve_objective_ticket<'a>(
        &'a self,
        request: FakeTicketResolverRequest,
    ) -> FakeObjectiveProviderFuture<'a, FakeTicketResolverRawResponse>;
}

pub trait FakeLocalImplementationProvider: Send + Sync {
    fn complete_local_task<'a>(
        &'a self,
        request: ModelRequest,
    ) -> ProviderFuture<'a, ModelResponse>;
}

#[derive(Debug, Clone)]
pub struct FakePlannerProvider {
    ledger: SharedProviderCallLedger,
    responses: Arc<Mutex<VecDeque<ProviderResult<FakePlannerRawResponse>>>>,
}

impl FakePlannerProvider {
    pub fn new(ledger: SharedProviderCallLedger) -> Self {
        Self {
            ledger,
            responses: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    pub fn push_text(&self, text: impl Into<String>) {
        self.responses
            .lock()
            .expect("fake planner response lock poisoned")
            .push_back(Ok(FakePlannerRawResponse {
                response_id: None,
                text: text.into(),
            }));
    }
}

impl FakeRemotePlanningProvider for FakePlannerProvider {
    fn plan_objective<'a>(
        &'a self,
        request: FakePlannerRequest,
    ) -> FakeObjectiveProviderFuture<'a, FakePlannerRawResponse> {
        Box::pin(async move {
            self.ledger
                .lock()
                .expect("provider call ledger lock poisoned")
                .record_planner(request);
            self.responses
                .lock()
                .expect("fake planner response lock poisoned")
                .pop_front()
                .unwrap_or_else(|| {
                    Err(ProviderError::new(
                        ProviderErrorKind::BadRequest,
                        "fake planner has no queued response",
                    ))
                })
        })
    }
}

#[derive(Debug, Clone)]
pub struct FakeTicketResolverProvider {
    ledger: SharedProviderCallLedger,
    responses: Arc<Mutex<VecDeque<ProviderResult<FakeTicketResolverRawResponse>>>>,
}

impl FakeTicketResolverProvider {
    pub fn new(ledger: SharedProviderCallLedger) -> Self {
        Self {
            ledger,
            responses: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    pub fn push_text(&self, text: impl Into<String>) {
        self.responses
            .lock()
            .expect("fake resolver response lock poisoned")
            .push_back(Ok(FakeTicketResolverRawResponse {
                response_id: None,
                text: text.into(),
            }));
    }
}

impl FakeRemoteTicketResolverProvider for FakeTicketResolverProvider {
    fn resolve_objective_ticket<'a>(
        &'a self,
        request: FakeTicketResolverRequest,
    ) -> FakeObjectiveProviderFuture<'a, FakeTicketResolverRawResponse> {
        Box::pin(async move {
            self.ledger
                .lock()
                .expect("provider call ledger lock poisoned")
                .record_resolver(request);
            self.responses
                .lock()
                .expect("fake resolver response lock poisoned")
                .pop_front()
                .unwrap_or_else(|| {
                    Err(ProviderError::new(
                        ProviderErrorKind::BadRequest,
                        "fake resolver has no queued response",
                    ))
                })
        })
    }
}

#[derive(Debug, Clone)]
pub struct FakeLocalProvider {
    ledger: SharedProviderCallLedger,
    responses: Arc<Mutex<VecDeque<ProviderResult<ModelResponse>>>>,
}

impl FakeLocalProvider {
    pub fn new(ledger: SharedProviderCallLedger) -> Self {
        Self {
            ledger,
            responses: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    pub fn push_text(&self, provider: impl Into<String>, text: impl Into<String>) {
        self.responses
            .lock()
            .expect("fake local response lock poisoned")
            .push_back(Ok(ModelResponse {
                provider: provider.into(),
                model: "fake-local-model".to_string(),
                response_id: None,
                text: text.into(),
            }));
    }
}

impl FakeLocalImplementationProvider for FakeLocalProvider {
    fn complete_local_task<'a>(
        &'a self,
        request: ModelRequest,
    ) -> ProviderFuture<'a, ModelResponse> {
        Box::pin(async move {
            self.ledger
                .lock()
                .expect("provider call ledger lock poisoned")
                .record_local(request);
            self.responses
                .lock()
                .expect("fake local response lock poisoned")
                .pop_front()
                .unwrap_or_else(|| {
                    Err(ProviderError::new(
                        ProviderErrorKind::BadRequest,
                        "fake local provider has no queued response",
                    ))
                })
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeClock {
    current: String,
}

impl FakeClock {
    pub fn new(current: impl Into<String>) -> Self {
        Self {
            current: current.into(),
        }
    }

    pub fn now(&self) -> String {
        self.current.clone()
    }

    pub fn set(&mut self, current: impl Into<String>) {
        self.current = current.into();
    }
}

#[derive(Debug, Default)]
pub struct DeterministicObjectiveIds {
    objective_sequence: usize,
    task_sequence: usize,
    ticket_sequence: usize,
}

impl DeterministicObjectiveIds {
    pub fn objective(&mut self) -> ObjectiveId {
        self.objective_sequence += 1;
        ObjectiveId::parse(deterministic_id("objective_", self.objective_sequence)).unwrap()
    }

    pub fn task(&mut self) -> TaskId {
        self.task_sequence += 1;
        TaskId::parse(deterministic_id("task_", self.task_sequence)).unwrap()
    }

    pub fn ticket(&mut self) -> TicketId {
        self.ticket_sequence += 1;
        TicketId::parse(deterministic_id("ticket_", self.ticket_sequence)).unwrap()
    }
}

fn deterministic_id(prefix: &str, sequence: usize) -> String {
    const ALPHABET: &[u8] = b"123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let ch = ALPHABET
        .get(sequence.saturating_sub(1))
        .copied()
        .unwrap_or(b'Z') as char;
    format!("{prefix}01ARZ3NDEKTSV4RRFFQ69G5FA{ch}")
}

#[derive(Debug)]
pub struct FakeOllamaServer {
    inner: FakeHttpServer,
}

#[derive(Debug)]
pub struct FakeOpenAiServer {
    inner: FakeHttpServer,
}

impl FakeOllamaServer {
    pub fn scripted<I, T>(model: &str, responses: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<String>,
    {
        let responses = responses
            .into_iter()
            .map(|text| {
                FakeHttpResponse::json(
                    200,
                    json!({
                        "model": model,
                        "response": text.into(),
                        "done": true,
                    }),
                )
            })
            .collect::<Vec<_>>();

        Self {
            inner: FakeHttpServer::start(vec![
                FakeHttpRoute::new(
                    "GET",
                    "/api/tags",
                    FakeHttpResponse::json(
                        200,
                        json!({
                            "models": [
                                {
                                    "name": model,
                                    "model": model,
                                }
                            ]
                        }),
                    ),
                ),
                FakeHttpRoute::sequence("POST", "/api/generate", responses).with_expectation(
                    FakeHttpExpectation::OllamaGenerate {
                        model: model.to_string(),
                    },
                ),
            ])
            .expect("start fake Ollama server"),
        }
    }

    pub fn http_error(model: &str, status: u16, message: &str) -> Self {
        Self {
            inner: FakeHttpServer::start(vec![
                FakeHttpRoute::new(
                    "GET",
                    "/api/tags",
                    FakeHttpResponse::json(
                        200,
                        json!({
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
                    FakeHttpResponse::json(status, json!({ "error": message })),
                )
                .with_expectation(FakeHttpExpectation::OllamaGenerate {
                    model: model.to_string(),
                }),
            ])
            .expect("start fake Ollama server"),
        }
    }

    pub fn base_url(&self) -> String {
        self.inner.base_url()
    }

    pub fn requests(&self) -> Vec<FakeHttpRequest> {
        self.inner.requests()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_call_ledger_counts_roles_independently() {
        let mut ids = DeterministicObjectiveIds::default();
        let objective_id = ids.objective();
        let ticket_id = ids.ticket();
        let mut ledger = ProviderCallLedger::default();

        ledger.record_planner(FakePlannerRequest {
            objective_id: objective_id.clone(),
            prompt: "build the tool".to_string(),
            model: "planner-model".to_string(),
        });
        ledger.record_resolver(FakeTicketResolverRequest {
            objective_id,
            ticket_id,
            prompt: "diagnose ticket".to_string(),
            model: "resolver-model".to_string(),
        });
        ledger.record_local(ModelRequest {
            model: "local-model".to_string(),
            system: None,
            input: "implement task".to_string(),
            temperature: None,
            max_output_tokens: None,
            metadata: Default::default(),
        });

        ledger.assert_counts(1, 1, 1);
        assert_eq!(ledger.planner_calls()[0].model, "planner-model");
        assert_eq!(ledger.resolver_calls()[0].model, "resolver-model");
        assert_eq!(ledger.local_calls()[0].model, "local-model");
    }

    #[test]
    fn fake_clock_and_deterministic_ids_are_stable() {
        let mut clock = FakeClock::new("2026-05-14T12:00:00Z");
        assert_eq!(clock.now(), "2026-05-14T12:00:00Z");
        clock.set("2026-05-14T12:01:00Z");
        assert_eq!(clock.now(), "2026-05-14T12:01:00Z");

        let mut ids = DeterministicObjectiveIds::default();
        assert_eq!(
            ids.objective().as_str(),
            "objective_01ARZ3NDEKTSV4RRFFQ69G5FA1"
        );
        assert_eq!(ids.task().as_str(), "task_01ARZ3NDEKTSV4RRFFQ69G5FA1");
        assert_eq!(ids.ticket().as_str(), "ticket_01ARZ3NDEKTSV4RRFFQ69G5FA1");
    }
}

impl FakeOpenAiServer {
    pub fn scripted<I, R, T>(model: &str, responses: I) -> Self
    where
        I: IntoIterator<Item = (R, T)>,
        R: Into<String>,
        T: Into<String>,
    {
        let responses = responses
            .into_iter()
            .map(|(response_id, text)| {
                FakeHttpResponse::json(
                    200,
                    json!({
                        "model": model,
                        "id": response_id.into(),
                        "status": "completed",
                        "output": [
                            {
                                "type": "message",
                                "content": [
                                    {
                                        "type": "output_text",
                                        "text": text.into(),
                                    }
                                ]
                            }
                        ],
                    }),
                )
            })
            .collect::<Vec<_>>();

        Self {
            inner: FakeHttpServer::start(vec![
                FakeHttpRoute::new(
                    "GET",
                    "/models",
                    FakeHttpResponse::json(
                        200,
                        json!({
                            "data": [
                                {
                                    "id": model,
                                }
                            ]
                        }),
                    ),
                ),
                FakeHttpRoute::sequence("POST", "/responses", responses).with_expectation(
                    FakeHttpExpectation::OpenAiResponses {
                        model: model.to_string(),
                    },
                ),
            ])
            .expect("start fake OpenAI-compatible server"),
        }
    }

    pub fn base_url(&self) -> String {
        self.inner.base_url()
    }

    pub fn requests(&self) -> Vec<FakeHttpRequest> {
        self.inner.requests()
    }
}
