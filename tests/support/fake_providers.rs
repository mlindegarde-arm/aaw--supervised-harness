use harness::providers::{
    FakeHttpExpectation, FakeHttpRequest, FakeHttpResponse, FakeHttpRoute, FakeHttpServer,
};
use serde_json::json;

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
