//! Fragment requests. One HTTP client per IP family, so a family mismatch cannot be expressed.

use dalsegno_core::redact::Redacted;
use dalsegno_core::stream::IpFamily;
use reqwest::Client;
use reqwest::header::HeaderMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Several fragment durations, because a request just past the head is held open.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;

/// Why a request produced no response. Carries no URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    /// No connection or no complete response in time.
    #[error("timed out")]
    Timeout,
    /// The connection failed.
    #[error("connection failed")]
    Connect,
    /// The body exceeded the size limit.
    #[error("response too large")]
    TooLarge,
    /// Any other client failure.
    #[error("request failed")]
    Other,
}

impl From<reqwest::Error> for TransportError {
    fn from(error: reqwest::Error) -> Self {
        if error.is_timeout() {
            Self::Timeout
        } else if error.is_connect() {
            Self::Connect
        } else {
            Self::Other
        }
    }
}

/// One response from the origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FragmentResponse {
    /// HTTP status.
    pub status: u16,
    /// `X-Head-Seqnum`.
    pub head_seqnum: Option<u64>,
    /// `X-Sequence-Num`.
    pub sequence_num: Option<u64>,
    /// `X-Head-Time-Millis`, the stream time of the head.
    pub head_time_ms: Option<u64>,
    /// `X-Walltime-Ms`, the server's clock at the time of the response.
    pub walltime_ms: Option<u64>,
    /// Response body.
    pub body: Vec<u8>,
}

/// Where each family's connections originate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BindAddresses {
    /// Local address for IPv4.
    pub v4: IpAddr,
    /// Local address for IPv6.
    pub v6: IpAddr,
}

impl Default for BindAddresses {
    fn default() -> Self {
        Self {
            v4: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            v6: IpAddr::V6(Ipv6Addr::UNSPECIFIED),
        }
    }
}

/// Fetches fragments by `sq`.
#[derive(Debug, Clone)]
pub struct FragmentClient {
    v4: Client,
    v6: Client,
}

impl FragmentClient {
    /// Builds one client per family.
    pub fn new(bind: BindAddresses) -> Result<Self, TransportError> {
        Ok(Self {
            v4: build_client(bind.v4)?,
            v6: build_client(bind.v6)?,
        })
    }

    /// Requests fragment `sq` from a URL resolved for `family`.
    pub async fn get(
        &self,
        url: &Redacted<String>,
        sq: u64,
        family: IpFamily,
    ) -> Result<FragmentResponse, TransportError> {
        let client = match family {
            IpFamily::V4 => &self.v4,
            IpFamily::V6 => &self.v6,
        };
        let mut response = client.get(fragment_url(url.expose(), sq)).send().await?;
        let status = response.status().as_u16();
        let head_seqnum = number_header(response.headers(), "x-head-seqnum");
        let sequence_num = number_header(response.headers(), "x-sequence-num");
        let head_time_ms = number_header(response.headers(), "x-head-time-millis");
        let walltime_ms = number_header(response.headers(), "x-walltime-ms");
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if body.len() + chunk.len() > MAX_BODY_BYTES {
                return Err(TransportError::TooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(FragmentResponse {
            status,
            head_seqnum,
            sequence_num,
            head_time_ms,
            walltime_ms,
            body,
        })
    }
}

fn build_client(bind: IpAddr) -> Result<Client, TransportError> {
    Ok(Client::builder()
        .local_address(bind)
        .referer(false)
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()?)
}

fn fragment_url(base: &str, sq: u64) -> String {
    let separator = if base.contains('?') { '&' } else { '?' };
    format!("{base}{separator}sq={sq}")
}

fn number_header(headers: &HeaderMap, name: &str) -> Option<u64> {
    headers.get(name)?.to_str().ok()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client() -> FragmentClient {
        let loopback = IpAddr::V4(Ipv4Addr::LOCALHOST);
        FragmentClient::new(BindAddresses {
            v4: loopback,
            v6: loopback,
        })
        .unwrap()
    }

    fn url(server: &MockServer) -> Redacted<String> {
        Redacted::new(format!("{}/videoplayback?itag=140", server.uri()))
    }

    #[test]
    fn appends_sq_to_a_query() {
        assert_eq!(fragment_url("http://h/v?a=1", 7), "http://h/v?a=1&sq=7");
    }

    #[test]
    fn starts_a_query_when_there_is_none() {
        assert_eq!(fragment_url("http://h/v", 7), "http://h/v?sq=7");
    }

    #[test]
    fn default_binds_are_unspecified_addresses_of_each_family() {
        let bind = BindAddresses::default();

        assert!(bind.v4.is_ipv4() && bind.v4.is_unspecified());
        assert!(bind.v6.is_ipv6() && bind.v6.is_unspecified());
    }

    #[tokio::test]
    async fn returns_status_headers_and_body() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/videoplayback"))
            .and(query_param("itag", "140"))
            .and(query_param("sq", "7"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("X-Head-Seqnum", "99")
                    .insert_header("X-Sequence-Num", "7")
                    .insert_header("X-Head-Time-Millis", "12345678")
                    .insert_header("X-Walltime-Ms", "1700000000000")
                    .set_body_bytes(b"fragment".to_vec()),
            )
            .mount(&server)
            .await;

        let response = client().get(&url(&server), 7, IpFamily::V4).await.unwrap();

        assert_eq!(
            response,
            FragmentResponse {
                status: 200,
                head_seqnum: Some(99),
                sequence_num: Some(7),
                head_time_ms: Some(12_345_678),
                walltime_ms: Some(1_700_000_000_000),
                body: b"fragment".to_vec(),
            }
        );
    }

    #[tokio::test]
    async fn error_statuses_are_responses_not_failures() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404).insert_header("X-Head-Seqnum", "5"))
            .mount(&server)
            .await;

        let response = client().get(&url(&server), 7, IpFamily::V4).await.unwrap();

        assert_eq!(response.status, 404);
        assert_eq!(response.head_seqnum, Some(5));
        assert!(response.body.is_empty());
    }

    #[tokio::test]
    async fn malformed_headers_are_ignored() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).insert_header("X-Head-Seqnum", "soon"))
            .mount(&server)
            .await;

        let response = client().get(&url(&server), 7, IpFamily::V4).await.unwrap();

        assert_eq!(response.head_seqnum, None);
    }

    #[tokio::test]
    async fn oversized_bodies_are_rejected() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0; MAX_BODY_BYTES + 1]))
            .mount(&server)
            .await;

        let error = client()
            .get(&url(&server), 7, IpFamily::V4)
            .await
            .unwrap_err();

        assert_eq!(error, TransportError::TooLarge);
    }

    #[tokio::test]
    async fn connection_failures_carry_no_url() {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let target = Redacted::new(format!("http://{address}/videoplayback?itag=140"));

        let error = client().get(&target, 7, IpFamily::V4).await.unwrap_err();

        assert_eq!(error, TransportError::Connect);
        assert!(!error.to_string().contains("videoplayback"));
    }
}
