//! Plex over its HTTP API (D-13). Apart from the rescan request, access is read-only.

use dalsegno_core::redact::Redacted;
use serde::Deserialize;

const TOKEN_HEADER: &str = "X-Plex-Token";

/// Why Plex could not be reached. Details are dropped, as errors from the client can carry the
/// request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PlexError {
    /// The request did not complete.
    #[error("Plex did not answer")]
    Unreachable,
    /// Plex refused the token.
    #[error("Plex refused the token")]
    Unauthorised,
    /// Plex answered with a failure.
    #[error("Plex answered with status {0}")]
    Status(u16),
    /// The answer was not what Plex sends.
    #[error("Plex gave an unreadable answer")]
    Unreadable,
}

/// A Plex server.
#[derive(Debug, Clone)]
pub struct Plex {
    client: reqwest::Client,
    base: String,
    token: Redacted<String>,
}

#[derive(Deserialize)]
struct Answer {
    #[serde(rename = "MediaContainer")]
    container: Container,
}

#[derive(Deserialize)]
struct Container {
    #[serde(rename = "Metadata", default)]
    items: Vec<Item>,
}

#[derive(Deserialize)]
struct Item {
    #[serde(rename = "Media", default)]
    media: Vec<Media>,
}

#[derive(Deserialize)]
struct Media {
    #[serde(rename = "Part", default)]
    parts: Vec<Part>,
}

#[derive(Deserialize)]
struct Part {
    file: Option<String>,
}

impl Plex {
    /// Addresses a server. The token travels in a header, so it stays out of URLs and logs.
    pub fn new(base: &str, token: Redacted<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            base: base.trim_end_matches('/').to_string(),
            token,
        }
    }

    /// Asks Plex to scan a library section.
    pub async fn refresh(&self, section: u32) -> Result<(), PlexError> {
        let path = format!("/library/sections/{section}/refresh");
        self.get(&path).await.map(|_| ())
    }

    /// Returns whether a section holds a file ending in `relative`, such as
    /// `<name>/<name>.mp4`. Plex can mount the library elsewhere, so the root is not compared.
    pub async fn has_file(&self, section: u32, relative: &str) -> Result<bool, PlexError> {
        let path = format!("/library/sections/{section}/all");
        let body = self.get(&path).await?;
        let answer: Answer = serde_json::from_str(&body).map_err(|_| PlexError::Unreadable)?;
        let wanted = format!("/{relative}");
        let mut files = answer
            .container
            .items
            .iter()
            .flat_map(|item| &item.media)
            .flat_map(|media| &media.parts)
            .filter_map(|part| part.file.as_deref());
        Ok(files.any(|file| file.ends_with(&wanted)))
    }

    async fn get(&self, path: &str) -> Result<String, PlexError> {
        let response = self
            .client
            .get(format!("{}{path}", self.base))
            .header(TOKEN_HEADER, self.token.expose())
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|_| PlexError::Unreachable)?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(PlexError::Unauthorised);
        }
        if !status.is_success() {
            return Err(PlexError::Status(status.as_u16()));
        }
        response.text().await.map_err(|_| PlexError::Unreadable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn plex(server: &MockServer) -> Plex {
        Plex::new(
            &format!("{}/", server.uri()),
            Redacted::new("token-value".to_string()),
        )
    }

    fn library() -> serde_json::Value {
        json!({"MediaContainer": {"Metadata": [
            {"Media": [{"Part": [{"file": "/data/video/Example/Example.mp4"}]}]},
            {"Media": [{"Part": [{}]}]},
            {},
        ]}})
    }

    async fn serving(body: serde_json::Value) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/library/sections/3/all"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn a_refresh_carries_the_token_in_a_header() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/library/sections/3/refresh"))
            .and(header(TOKEN_HEADER, "token-value"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        plex(&server).refresh(3).await.unwrap();
    }

    #[tokio::test]
    async fn the_token_stays_out_of_the_url() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        plex(&server).refresh(3).await.unwrap();

        let requests = server.received_requests().await.unwrap();
        assert!(!requests[0].url.as_str().contains("token-value"));
    }

    #[tokio::test]
    async fn a_file_is_found_wherever_plex_mounts_the_library() {
        let server = serving(library()).await;

        let found = plex(&server).has_file(3, "Example/Example.mp4").await;

        assert_eq!(found, Ok(true));
    }

    #[tokio::test]
    async fn a_name_that_only_ends_alike_is_not_the_file() {
        let server = serving(library()).await;

        let found = plex(&server).has_file(3, "ample/Example.mp4").await;

        assert_eq!(found, Ok(false));
    }

    #[tokio::test]
    async fn an_empty_section_holds_nothing() {
        let server = serving(json!({"MediaContainer": {}})).await;

        assert_eq!(plex(&server).has_file(3, "a/a.mp4").await, Ok(false));
    }

    #[tokio::test]
    async fn a_refused_token_is_named() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        assert_eq!(plex(&server).refresh(3).await, Err(PlexError::Unauthorised));
    }

    #[tokio::test]
    async fn other_failures_keep_their_status() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        assert_eq!(plex(&server).refresh(3).await, Err(PlexError::Status(500)));
    }

    #[tokio::test]
    async fn an_answer_that_is_not_json_is_unreadable() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<xml/>"))
            .mount(&server)
            .await;

        assert_eq!(
            plex(&server).has_file(3, "a/a.mp4").await,
            Err(PlexError::Unreadable)
        );
    }

    #[tokio::test]
    async fn an_unreachable_server_is_named() {
        let plex = Plex::new("http://127.0.0.1:1", Redacted::new(String::new()));

        assert_eq!(plex.refresh(3).await, Err(PlexError::Unreachable));
    }

    #[test]
    fn the_token_never_prints() {
        let plex = Plex::new("http://x", Redacted::new("token-value".to_string()));

        assert!(!format!("{plex:?}").contains("token-value"));
    }
}
