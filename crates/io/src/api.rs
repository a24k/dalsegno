//! A client of the service's own API, which the CLI talks through (FR-C1).

use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(30);
const API_PATH: &str = "/api/v1";

/// Why a call failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ApiCallError {
    /// No service answered at this address.
    #[error("no service answers at {0}, is dalsegno serve running?")]
    Unreachable(String),
    /// The answer was not the API's.
    #[error("the service gave an unreadable answer")]
    Unreadable,
    /// The service refused, with its stable code.
    #[error("{message} ({code})")]
    Refused {
        /// Stable code.
        code: String,
        /// Readable message.
        message: String,
    },
}

#[derive(Debug, Deserialize)]
struct Failure {
    code: String,
    message: String,
}

#[derive(Debug, Deserialize)]
struct Envelope {
    data: Value,
    error: Option<Failure>,
}

/// A running service.
#[derive(Debug, Clone)]
pub struct ApiClient {
    client: reqwest::Client,
    base: String,
}

impl ApiClient {
    /// Addresses a service.
    pub fn new(base: &str) -> Result<Self, ApiCallError> {
        let base = base.trim_end_matches('/').to_string();
        let client = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .build()
            .map_err(|_| ApiCallError::Unreachable(base.clone()))?;
        Ok(Self { client, base })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{API_PATH}{path}", self.base)
    }

    async fn read(&self, request: reqwest::RequestBuilder) -> Result<Value, ApiCallError> {
        let response = request
            .send()
            .await
            .map_err(|_| ApiCallError::Unreachable(self.base.clone()))?;
        let body = response
            .text()
            .await
            .map_err(|_| ApiCallError::Unreadable)?;
        let envelope: Envelope =
            serde_json::from_str(&body).map_err(|_| ApiCallError::Unreadable)?;
        match envelope.error {
            Some(failure) => Err(ApiCallError::Refused {
                code: failure.code,
                message: failure.message,
            }),
            None => Ok(envelope.data),
        }
    }

    /// Reads an endpoint, and returns its data.
    pub async fn get(&self, path: &str) -> Result<Value, ApiCallError> {
        self.read(self.client.get(self.url(path))).await
    }

    /// Calls an endpoint with no body, and returns its data.
    pub async fn post(&self, path: &str) -> Result<Value, ApiCallError> {
        self.read(self.client.post(self.url(path))).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn answering(status: u16, body: Value) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(path("/api/v1/streams"))
            .respond_with(ResponseTemplate::new(status).set_body_json(body))
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn data_comes_out_of_the_envelope() {
        let body = json!({ "success": true, "data": [1], "error": null, "meta": null });
        let server = answering(200, body).await;

        let client = ApiClient::new(&format!("{}/", server.uri())).unwrap();

        assert_eq!(client.get("/streams").await.unwrap(), json!([1]));
    }

    #[tokio::test]
    async fn a_refusal_keeps_its_code() {
        let body = json!({
            "success": false,
            "data": null,
            "error": { "code": "invalid_request", "message": "the stream has ended" },
            "meta": null,
        });
        let server = answering(409, body).await;

        let error = ApiClient::new(&server.uri())
            .unwrap()
            .get("/streams")
            .await
            .unwrap_err();

        assert_eq!(error.to_string(), "the stream has ended (invalid_request)");
    }

    #[tokio::test]
    async fn a_post_is_a_post() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/streams/1/stop"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": 1 })))
            .expect(1)
            .mount(&server)
            .await;

        let data = ApiClient::new(&server.uri())
            .unwrap()
            .post("/streams/1/stop")
            .await
            .unwrap();

        assert_eq!(data, json!(1));
    }

    #[tokio::test]
    async fn something_else_at_that_address_is_unreadable() {
        let server = MockServer::start().await;
        Mock::given(path("/api/v1/streams"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<html>"))
            .mount(&server)
            .await;

        let error = ApiClient::new(&server.uri())
            .unwrap()
            .get("/streams")
            .await
            .unwrap_err();

        assert_eq!(error, ApiCallError::Unreadable);
    }

    #[tokio::test]
    async fn no_service_says_so() {
        let client = ApiClient::new("http://127.0.0.1:9").unwrap();

        let error = client.get("/streams").await.unwrap_err();

        assert!(matches!(error, ApiCallError::Unreachable(_)));
        assert!(error.to_string().contains("dalsegno serve"));
    }
}
