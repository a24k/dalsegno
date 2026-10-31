//! Pure decisions of the fetch path: response classification, lease expiry and back-off.

use std::time::Duration;

/// A lease is replaced this long before it expires (FR-A2).
pub const LEASE_MARGIN_SECONDS: u64 = 1800;

const BACKOFF_CAP: Duration = Duration::from_secs(60);
const MAX_DOUBLINGS: u32 = 16;

/// What a fragment response means for the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The fragment was served.
    Success,
    /// A 200 with no body: an IP family mismatch or a soft block.
    Empty,
    /// The URL is expired or refused.
    RefreshUrl,
    /// The fragment is past the head and may exist later.
    NotYet,
    /// The fragment has left the DVR.
    Lost,
    /// A temporary failure: wait and repeat.
    Backoff,
    /// A status with no defined handling.
    Unexpected(u16),
}

/// The facts of one response needed for classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResponseFacts {
    /// HTTP status.
    pub status: u16,
    /// Body length in bytes.
    pub body_len: usize,
    /// `X-Head-Seqnum`, when present and numeric.
    pub head_seqnum: Option<u64>,
    /// `X-Sequence-Num`, when present and numeric.
    pub sequence_num: Option<u64>,
}

/// Classifies the response to a request for `sq`.
pub fn classify(sq: u64, facts: ResponseFacts) -> Outcome {
    match facts.status {
        200 if facts.body_len == 0 => Outcome::Empty,
        200 if facts.sequence_num.is_some_and(|served| served != sq) => Outcome::Backoff,
        200 => Outcome::Success,
        403 => Outcome::RefreshUrl,
        404 => match facts.head_seqnum {
            Some(head) if sq > head => Outcome::NotYet,
            Some(_) => Outcome::Lost,
            None => Outcome::Backoff,
        },
        429 | 500..=599 => Outcome::Backoff,
        status => Outcome::Unexpected(status),
    }
}

/// Reads the `expire` value from a query string or a `/expire/<n>/` path segment.
pub fn parse_expire(url: &str) -> Option<u64> {
    let from_query = url.split_once('?').and_then(|(_, query)| {
        query
            .split('&')
            .find_map(|pair| pair.strip_prefix("expire="))
    });
    let from_path = || {
        url.split_once("/expire/")
            .and_then(|(_, rest)| rest.split('/').next())
    };
    from_query.or_else(from_path)?.parse().ok()
}

/// Returns whether a lease expiring at `expire` can still be used at `now`, in Unix seconds.
pub fn lease_is_fresh(expire: u64, now: u64) -> bool {
    now.saturating_add(LEASE_MARGIN_SECONDS) < expire
}

/// Returns the delay before retry `attempt`, counting from 0, with equal jitter.
///
/// `jitter` is a random value in `0.0..=1.0`, passed in to keep this function pure.
pub fn backoff_delay(base: Duration, attempt: u32, jitter: f64) -> Duration {
    let doubled = base.saturating_mul(1 << attempt.min(MAX_DOUBLINGS));
    doubled
        .min(BACKOFF_CAP)
        .mul_f64(0.5 + 0.5 * jitter.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(status: u16, body_len: usize, head: Option<u64>) -> ResponseFacts {
        ResponseFacts {
            status,
            body_len,
            head_seqnum: head,
            sequence_num: None,
        }
    }

    #[test]
    fn body_on_200_is_success() {
        assert_eq!(classify(5, facts(200, 10, Some(9))), Outcome::Success);
    }

    #[test]
    fn empty_200_is_not_success() {
        assert_eq!(classify(5, facts(200, 0, Some(9))), Outcome::Empty);
    }

    #[test]
    fn wrong_fragment_is_retried() {
        let response = ResponseFacts {
            sequence_num: Some(6),
            ..facts(200, 10, Some(9))
        };

        assert_eq!(classify(5, response), Outcome::Backoff);
    }

    #[test]
    fn matching_sequence_number_is_success() {
        let response = ResponseFacts {
            sequence_num: Some(5),
            ..facts(200, 10, Some(9))
        };

        assert_eq!(classify(5, response), Outcome::Success);
    }

    #[test]
    fn forbidden_refreshes_the_url() {
        assert_eq!(classify(5, facts(403, 0, None)), Outcome::RefreshUrl);
    }

    #[test]
    fn not_found_past_head_is_not_yet() {
        assert_eq!(classify(10, facts(404, 0, Some(9))), Outcome::NotYet);
    }

    #[test]
    fn not_found_behind_head_is_lost() {
        assert_eq!(classify(5, facts(404, 0, Some(9))), Outcome::Lost);
        assert_eq!(classify(9, facts(404, 0, Some(9))), Outcome::Lost);
    }

    #[test]
    fn not_found_without_head_cannot_be_judged() {
        assert_eq!(classify(5, facts(404, 0, None)), Outcome::Backoff);
    }

    #[test]
    fn throttling_and_server_errors_back_off() {
        for status in [429, 500, 502, 503, 599] {
            assert_eq!(classify(5, facts(status, 0, None)), Outcome::Backoff);
        }
    }

    #[test]
    fn other_statuses_are_unexpected() {
        assert_eq!(classify(5, facts(410, 0, None)), Outcome::Unexpected(410));
        assert_eq!(classify(5, facts(204, 0, None)), Outcome::Unexpected(204));
    }

    #[test]
    fn reads_expire_from_query() {
        assert_eq!(parse_expire("https://h/v?a=1&expire=1700&b=2"), Some(1700));
        assert_eq!(parse_expire("https://h/v?expire=1700"), Some(1700));
    }

    #[test]
    fn reads_expire_from_path() {
        assert_eq!(parse_expire("https://h/v/expire/1700/ei/x"), Some(1700));
    }

    #[test]
    fn query_keys_must_match_exactly() {
        assert_eq!(parse_expire("https://h/v?noexpire=1700"), None);
        assert_eq!(parse_expire("https://h/v?expire=soon"), None);
        assert_eq!(parse_expire("https://h/v"), None);
    }

    #[test]
    fn lease_is_replaced_inside_the_margin() {
        assert!(lease_is_fresh(10_000, 10_000 - LEASE_MARGIN_SECONDS - 1));
        assert!(!lease_is_fresh(10_000, 10_000 - LEASE_MARGIN_SECONDS));
        assert!(!lease_is_fresh(10_000, 20_000));
        assert!(!lease_is_fresh(0, u64::MAX));
    }

    #[test]
    fn backoff_doubles_up_to_the_cap() {
        let base = Duration::from_secs(1);

        assert_eq!(backoff_delay(base, 0, 1.0), Duration::from_secs(1));
        assert_eq!(backoff_delay(base, 3, 1.0), Duration::from_secs(8));
        assert_eq!(backoff_delay(base, 30, 1.0), BACKOFF_CAP);
    }

    #[test]
    fn jitter_keeps_at_least_half_the_delay() {
        let base = Duration::from_secs(2);

        assert_eq!(backoff_delay(base, 0, 0.0), Duration::from_secs(1));
        assert_eq!(backoff_delay(base, 0, -5.0), Duration::from_secs(1));
        assert_eq!(backoff_delay(base, 0, 5.0), Duration::from_secs(2));
    }
}
