//! Sending notifications to a webhook (FR-J3).

use dalsegno_core::redact::Redacted;
use serde_json::{Value, json};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(30);
/// Who the messages come from, which the Mattermost message says first.
const SENDER: &str = "dalsegno";

/// How serious a notification is, which Mattermost shows as the attachment's colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Something came back or went well.
    Good,
    /// Something to look at.
    Notice,
    /// Something was lost or stopped.
    Bad,
}

impl Tone {
    fn colour(self) -> &'static str {
        match self {
            Self::Good => "#3DB887",
            Self::Notice => "#FFBC1F",
            Self::Bad => "#D24B4E",
        }
    }
}

/// A notification, worded but not yet marked up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// What happened, such as "Change seen".
    pub event: String,
    /// The stream it happened to, if any.
    pub subject: Option<String>,
    /// The details.
    pub text: String,
    /// Where to act on it.
    pub link: Option<Link>,
    /// How serious it is.
    pub tone: Tone,
}

impl Message {
    /// What happened and to what, in one line, such as "Change seen: Example TV".
    pub fn title(&self) -> String {
        match &self.subject {
            Some(subject) => format!("{}: {subject}", self.event),
            None => self.event.clone(),
        }
    }
}

/// A link that reads as words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// What following it does, such as "Open the change".
    pub label: String,
    /// Where it goes.
    pub url: String,
}

/// How a webhook wants its message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebhookFormat {
    /// `{"text": ...}` in Slack's mrkdwn.
    Slack,
    /// `{"text": ...}` in Markdown, which Mattermost reads and Slack does not.
    Mattermost,
    /// The message as a plain body, as ntfy reads it.
    Ntfy,
}

impl WebhookFormat {
    /// Reads a setting.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "slack" => Some(Self::Slack),
            "mattermost" => Some(Self::Mattermost),
            "ntfy" => Some(Self::Ntfy),
            _ => None,
        }
    }

    /// Marks a message up for this format: the title in bold, then the text, then the link.
    /// Mattermost is sent as an attachment instead, and this is its form as one message.
    pub fn markup(self, message: &Message) -> String {
        let (title, text) = match self {
            Self::Slack => (
                format!("*{}*", escape_mrkdwn(&message.title())),
                escape_mrkdwn(&message.text),
            ),
            Self::Mattermost => (
                format!("**{}**", escape_markdown(&message.title())),
                escape_markdown(&message.text),
            ),
            Self::Ntfy => (message.title(), message.text.clone()),
        };
        let link = message.link.as_ref().map(|link| match self {
            Self::Slack => format!(
                "<{}|{}>",
                escape_mrkdwn(&link.url).replace('|', "%7C"),
                escape_mrkdwn(&link.label)
            ),
            Self::Mattermost => format!(
                "[{}]({})",
                escape_markdown(&link.label),
                link.url
                    .replace(' ', "%20")
                    .replace('(', "%28")
                    .replace(')', "%29")
            ),
            Self::Ntfy => format!("{}: {}", link.label, link.url),
        });
        [Some(title), Some(text), link]
            .into_iter()
            .flatten()
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// The Mattermost payload: who it is from and about in the message, and what happened in an
/// attachment coloured by its tone, with the whole line as the fallback for push notifications.
fn attachment_payload(message: &Message) -> Value {
    let header = match &message.subject {
        // Code spans have no escape and end at a line break, so those are swapped out.
        Some(subject) => format!(
            "**{SENDER}** `{}`",
            subject.replace('`', "'").replace(['\n', '\r'], " ")
        ),
        None => format!("**{SENDER}**"),
    };
    let attachment = json!({
        "fallback": message.title(),
        "color": message.tone.colour(),
        "title": message.event,
        "text": escape_markdown(&message.text),
        "footer": format!("{SENDER} {}", env!("CARGO_PKG_VERSION")),
    });
    // A link is added only when there is one, as some receivers refuse a null.
    let attachment = match (&message.link, attachment) {
        (Some(link), Value::Object(fields)) => Value::Object(
            fields
                .into_iter()
                .chain([("title_link".to_string(), json!(link.url))])
                .collect(),
        ),
        (_, attachment) => attachment,
    };
    json!({ "text": header, "attachments": [attachment] })
}

/// Slack reads `&`, `<` and `>` as control characters, and has no other escape, so a `*` in a
/// stream name can still end the bold early.
fn escape_mrkdwn(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Escapes the characters that would start emphasis, code, links or HTML.
fn escape_markdown(text: &str) -> String {
    text.chars()
        .flat_map(|character| {
            let is_special = matches!(
                character,
                '\\' | '*' | '_' | '`' | '~' | '[' | ']' | '<' | '>'
            );
            is_special.then_some('\\').into_iter().chain([character])
        })
        .collect()
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
    pub async fn send(&self, message: &Message) -> Result<(), WebhookError> {
        let request = self.client.post(self.url.expose());
        let request = match self.format {
            WebhookFormat::Slack => request
                .header("Content-Type", "application/json")
                .body(json!({ "text": self.format.markup(message) }).to_string()),
            WebhookFormat::Mattermost => request
                .header("Content-Type", "application/json")
                .body(attachment_payload(message).to_string()),
            WebhookFormat::Ntfy => request.body(self.format.markup(message)),
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

    fn message(title: &str, text: &str) -> Message {
        Message {
            event: title.to_string(),
            subject: None,
            text: text.to_string(),
            link: Some(Link {
                label: "Open the change".to_string(),
                url: "https://dalsegno.example/changes/7".to_string(),
            }),
            tone: Tone::Notice,
        }
    }

    fn hello() -> Message {
        Message {
            link: None,
            ..message("Hello", "world")
        }
    }

    #[tokio::test]
    async fn slack_gets_json() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(body_json(json!({ "text": "*Hello*\nworld" })))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        hook(&server, WebhookFormat::Slack)
            .send(&hello())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn mattermost_gets_an_attachment() {
        let server = MockServer::start().await;
        let version = env!("CARGO_PKG_VERSION");
        Mock::given(method("POST"))
            .and(body_json(json!({
                "text": "**dalsegno**",
                "attachments": [{
                    "fallback": "Hello",
                    "color": "#FFBC1F",
                    "title": "Hello",
                    "text": "world",
                    "footer": format!("dalsegno {version}"),
                }],
            })))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        hook(&server, WebhookFormat::Mattermost)
            .send(&hello())
            .await
            .unwrap();
    }

    #[test]
    fn mattermost_names_the_stream_and_links_the_title() {
        let change = Message {
            event: "Change seen".to_string(),
            subject: Some("Example `TV`\n".to_string()),
            tone: Tone::Bad,
            ..message("x", "a *b*")
        };

        let payload = attachment_payload(&change);

        assert_eq!(payload["text"], "**dalsegno** `Example 'TV' `");
        let attachment = &payload["attachments"][0];
        assert_eq!(attachment["fallback"], "Change seen: Example `TV`\n");
        assert_eq!(attachment["color"], "#D24B4E");
        assert_eq!(attachment["title"], "Change seen");
        assert_eq!(
            attachment["title_link"],
            "https://dalsegno.example/changes/7"
        );
        assert_eq!(attachment["text"], "a \\*b\\*");
    }

    #[test]
    fn each_tone_has_its_colour() {
        let colours = [Tone::Good, Tone::Notice, Tone::Bad].map(Tone::colour);

        assert_eq!(colours, ["#3DB887", "#FFBC1F", "#D24B4E"]);
    }

    #[tokio::test]
    async fn ntfy_gets_the_message_itself() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(body_string("Hello\nworld"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        hook(&server, WebhookFormat::Ntfy)
            .send(&hello())
            .await
            .unwrap();
    }

    #[test]
    fn slack_reads_the_link_as_words() {
        let text = WebhookFormat::Slack.markup(&message("Change seen: A & B", "x < y"));

        assert_eq!(
            text,
            "*Change seen: A &amp; B*\nx &lt; y\n<https://dalsegno.example/changes/7|Open the change>"
        );
    }

    #[test]
    fn mattermost_reads_the_link_as_words() {
        let text = WebhookFormat::Mattermost.markup(&message("Change seen: *A_B*", "x"));

        assert_eq!(
            text,
            "**Change seen: \\*A\\_B\\***\nx\n[Open the change](https://dalsegno.example/changes/7)"
        );
    }

    #[test]
    fn a_link_with_special_characters_stays_one_link() {
        let odd = Message {
            link: Some(Link {
                label: "Open [it]".to_string(),
                url: "https://dalsegno.example/a b(c)?d=1&e|f".to_string(),
            }),
            ..message("T", "x")
        };

        assert!(
            WebhookFormat::Slack
                .markup(&odd)
                .ends_with("<https://dalsegno.example/a b(c)?d=1&amp;e%7Cf|Open [it]>")
        );
        assert!(
            WebhookFormat::Mattermost
                .markup(&odd)
                .ends_with("[Open \\[it\\]](https://dalsegno.example/a%20b%28c%29?d=1&e|f)")
        );
    }

    #[test]
    fn mattermost_escapes_the_text_too() {
        let text = WebhookFormat::Mattermost.markup(&Message {
            link: None,
            ..message("T", "a\\b `c`")
        });

        assert_eq!(text, "**T**\na\\\\b \\`c\\`");
    }

    #[test]
    fn an_empty_text_leaves_no_blank_line() {
        let text = WebhookFormat::Slack.markup(&message("T", ""));

        assert_eq!(
            text,
            "*T*\n<https://dalsegno.example/changes/7|Open the change>"
        );
    }

    #[test]
    fn ntfy_keeps_plain_text() {
        let text = WebhookFormat::Ntfy.markup(&message("Change seen: *A*", "x"));

        assert_eq!(
            text,
            "Change seen: *A*\nx\nOpen the change: https://dalsegno.example/changes/7"
        );
    }

    #[tokio::test]
    async fn a_refusal_is_reported() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;

        let sent = hook(&server, WebhookFormat::Slack).send(&hello()).await;

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

        let sent = hook(&server, WebhookFormat::Slack).send(&hello()).await;

        assert_eq!(sent, Err(WebhookError::Status(302)));
    }

    #[tokio::test]
    async fn an_unreachable_webhook_does_not_show_its_url() {
        let url = Redacted::new("http://127.0.0.1:9/sesame".to_string());
        let hook = Webhook::new(url, WebhookFormat::Slack).unwrap();

        let error = hook.send(&hello()).await.unwrap_err();

        assert_eq!(error, WebhookError::Unreachable);
        assert!(!format!("{error} {error:?} {hook:?}").contains("sesame"));
    }

    #[test]
    fn each_format_is_read_from_its_name() {
        assert_eq!(WebhookFormat::parse("slack"), Some(WebhookFormat::Slack));
        assert_eq!(
            WebhookFormat::parse("mattermost"),
            Some(WebhookFormat::Mattermost)
        );
        assert_eq!(WebhookFormat::parse("ntfy"), Some(WebhookFormat::Ntfy));
        assert_eq!(WebhookFormat::parse("smoke signals"), None);
    }
}
