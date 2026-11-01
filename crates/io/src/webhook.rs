//! Sending notifications to a webhook (FR-J3).

use dalsegno_core::redact::Redacted;
use serde_json::json;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(30);

/// How a webhook wants its message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebhookFormat {
    /// `{"text": ...}`, which Slack and Mattermost both read.
    Slack,
    /// The message as a plain body, as ntfy reads it.
    Ntfy,
}

impl WebhookFormat {
    /// Reads a setting.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "slack" | "mattermost" => Some(Self::Slack),
            "ntfy" => Some(Self::Ntfy),
            _ => None,
        }
    }
}

/// Why a message was not delivered. Details are dropped, as the URL can embed a secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WebhookError {
    /// The request did not complete.
    #[error("the webhook did not answer")]
    Unreachable,
    /// The webhook refused the message.
    #[error("the webhook answered with status {0}")]
    Status(u16),
}

/// A webhook.
#[derive(Debug, Clone)]
pub struct Webhook {
    client: reqwest::Client,
    url: Redacted<String>,
    format: WebhookFormat,
}

impl Webhook {
    /// Addresses a webhook. Redirects are not followed, as the page at the end of one can
    /// answer 200 for a message nobody received.
    pub fn new(url: Redacted<String>, format: WebhookFormat) -> Result<Self, WebhookError> {
        let client = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| WebhookError::Unreachable)?;
        Ok(Self {
            client,
            url,
            format,
        })
    }

    /// Sends a message.
    pub async fn send(&self, text: &str) -> Result<(), WebhookError> {
        let request = self.client.post(self.url.expose());
        let request = match self.format {
            WebhookFormat::Slack => request
                .header("Content-Type", "application/json")
                .body(json!({ "text": text }).to_string()),
            WebhookFormat::Ntfy => request.body(text.to_string()),
        };
        let response = request
            .send()
            .await
            .map_err(|_| WebhookError::Unreachable)?;
        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            Err(WebhookError::Status(status.as_u16()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_json, body_string, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn hook(server: &MockServer, format: WebhookFormat) -> Webhook {
        Webhook::new(Redacted::new(server.uri()), format).unwrap()
    }

    #[tokio::test]
    async fn slack_gets_json() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(body_json(json!({ "text": "hello" })))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        hook(&server, WebhookFormat::Slack)
            .send("hello")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn ntfy_gets_the_message_itself() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(body_string("hello"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        hook(&server, WebhookFormat::Ntfy)
            .send("hello")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_refusal_is_reported() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;

        let sent = hook(&server, WebhookFormat::Slack).send("hello").await;

        assert_eq!(sent, Err(WebhookError::Status(503)));
    }

    // A proxy in front of a webhook can redirect to its login page, which answers 200.
    #[tokio::test]
    async fn a_redirect_is_not_a_delivery() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("Location", format!("{}/login", server.uri())),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let sent = hook(&server, WebhookFormat::Slack).send("hello").await;

        assert_eq!(sent, Err(WebhookError::Status(302)));
    }

    #[tokio::test]
    async fn an_unreachable_webhook_does_not_show_its_url() {
        let url = Redacted::new("http://127.0.0.1:9/sesame".to_string());
        let hook = Webhook::new(url, WebhookFormat::Slack).unwrap();

        let error = hook.send("hello").await.unwrap_err();

        assert_eq!(error, WebhookError::Unreachable);
        assert!(!format!("{error} {error:?} {hook:?}").contains("sesame"));
    }

    #[test]
    fn mattermost_reads_the_slack_format() {
        assert_eq!(
            WebhookFormat::parse("mattermost"),
            Some(WebhookFormat::Slack)
        );
        assert_eq!(WebhookFormat::parse("smoke signals"), None);
    }
}
