//! When and how long to wait before retrying.

use std::time::{Duration, SystemTime};

use reqwest::header::HeaderMap;

/// When and how long to wait before retrying. The defaults match TypeSafe's
/// official SDKs:
///
/// - `max_retries` (2): retries after the first attempt; 0 disables retrying
/// - `backoff_initial` (500 ms) and `backoff_max` (5 s): the delay doubles each
///   attempt up to the maximum
/// - `jitter` (0.25): the fraction of each delay randomly taken off, from 0 to 1
/// - `statuses`: HTTP statuses that are retried: 408, 429 and every 5xx, which
///   includes TypeSafe's 529 (overloaded)
/// - `respect_retry_after` (true): wait as long as `retry-after-ms` or
///   `retry-after` says
/// - `retry_connection_errors` (true): retry timeouts and failed connections
/// - `budget` (30 s): total time per call, including waits. A retry whose wait
///   would pass it is not attempted. `None` for no limit.
///
/// ```
/// use std::time::Duration;
/// use typesafe::RetryPolicy;
///
/// let patient = RetryPolicy {
///     max_retries: 5,
///     budget: Some(Duration::from_secs(60)),
///     ..RetryPolicy::default()
/// };
/// let never = RetryPolicy::disabled();
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct RetryPolicy {
    pub max_retries: u32,
    pub backoff_initial: Duration,
    pub backoff_max: Duration,
    pub jitter: f64,
    pub statuses: Vec<u16>,
    pub respect_retry_after: bool,
    pub retry_connection_errors: bool,
    pub budget: Option<Duration>,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        RetryPolicy {
            max_retries: 2,
            backoff_initial: Duration::from_millis(500),
            backoff_max: Duration::from_secs(5),
            jitter: 0.25,
            statuses: [408, 429].into_iter().chain(500..=599).collect(),
            respect_retry_after: true,
            retry_connection_errors: true,
            budget: Some(Duration::from_secs(30)),
        }
    }
}

impl RetryPolicy {
    /// No retries.
    pub fn disabled() -> Self {
        RetryPolicy {
            max_retries: 0,
            ..RetryPolicy::default()
        }
    }

    /// The wait before the next attempt, or `None` to stop. `attempts` counts
    /// the attempts made so far, the first included. `remaining` is what is
    /// left of the budget, if there is one.
    pub(crate) fn next_delay(
        &self,
        attempts: u32,
        failure: Failure,
        remaining: Option<Duration>,
    ) -> Option<Duration> {
        if attempts > self.max_retries {
            return None;
        }
        let delay = match failure {
            Failure::Status { status, .. } if !self.statuses.contains(&status) => return None,
            Failure::Transport if !self.retry_connection_errors => return None,
            Failure::Status {
                retry_after: Some(wait),
                ..
            } if self.respect_retry_after => wait,
            _ => self.backoff(attempts),
        };
        match remaining {
            Some(left) if delay >= left => None,
            _ => Some(delay),
        }
    }

    fn backoff(&self, attempts: u32) -> Duration {
        let doubling = 2u32.saturating_pow(attempts.saturating_sub(1));
        let exponential = self
            .backoff_initial
            .saturating_mul(doubling)
            .min(self.backoff_max);
        exponential.mul_f64(1.0 - fastrand::f64() * self.jitter.clamp(0.0, 1.0))
    }
}

/// A failed attempt, as far as retrying cares.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Failure {
    Status {
        status: u16,
        retry_after: Option<Duration>,
    },
    Transport,
}

/// `retry-after-ms` first, then `retry-after` as seconds or an HTTP date.
pub(crate) fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    let header = |name: &str| headers.get(name)?.to_str().ok().map(str::trim);
    let seconds = |raw: &str, scale: f64| {
        raw.parse::<f64>()
            .ok()
            .filter(|v| v.is_finite() && *v >= 0.0)
            .map(|v| Duration::from_secs_f64(v * scale))
    };
    if let Some(wait) = header("retry-after-ms").and_then(|raw| seconds(raw, 0.001)) {
        return Some(wait);
    }
    let raw = header("retry-after")?;
    seconds(raw, 1.0).or_else(|| {
        let at = httpdate::parse_http_date(raw).ok()?;
        Some(
            at.duration_since(SystemTime::now())
                .unwrap_or(Duration::ZERO),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    fn status(status: u16) -> Failure {
        Failure::Status {
            status,
            retry_after: None,
        }
    }

    fn no_jitter() -> RetryPolicy {
        RetryPolicy {
            jitter: 0.0,
            ..RetryPolicy::default()
        }
    }

    #[test]
    fn backs_off_exponentially_to_the_maximum() {
        let policy = RetryPolicy {
            max_retries: 10,
            budget: None,
            ..no_jitter()
        };
        let delays: Vec<u128> = (1..=6)
            .map(|n| policy.next_delay(n, status(529), None).unwrap().as_millis())
            .collect();
        assert_eq!(delays, [500, 1000, 2000, 4000, 5000, 5000]);
    }

    #[test]
    fn jitter_only_takes_time_off() {
        let policy = RetryPolicy::default();
        for _ in 0..100 {
            let delay = policy.next_delay(1, status(429), None).unwrap();
            assert!(delay <= Duration::from_millis(500));
            assert!(delay >= Duration::from_millis(375));
        }
    }

    #[test]
    fn stops_after_max_retries() {
        let policy = no_jitter();
        assert!(policy.next_delay(1, status(500), None).is_some());
        assert!(policy.next_delay(2, status(500), None).is_some());
        assert!(policy.next_delay(3, status(500), None).is_none());
        assert!(
            RetryPolicy::disabled()
                .next_delay(1, status(500), None)
                .is_none()
        );
    }

    #[test]
    fn retries_only_listed_statuses() {
        let policy = no_jitter();
        for retried in [408, 429, 500, 503, 529] {
            assert!(
                policy.next_delay(1, status(retried), None).is_some(),
                "{retried}"
            );
        }
        for not in [400, 401, 403, 404, 422] {
            assert!(policy.next_delay(1, status(not), None).is_none(), "{not}");
        }
    }

    #[test]
    fn connection_errors_follow_the_flag() {
        assert!(
            no_jitter()
                .next_delay(1, Failure::Transport, None)
                .is_some()
        );
        let off = RetryPolicy {
            retry_connection_errors: false,
            ..no_jitter()
        };
        assert!(off.next_delay(1, Failure::Transport, None).is_none());
    }

    #[test]
    fn honours_the_server_and_the_budget() {
        let asked = Failure::Status {
            status: 429,
            retry_after: Some(Duration::from_secs(3)),
        };
        let policy = no_jitter();
        assert_eq!(
            policy.next_delay(1, asked, None),
            Some(Duration::from_secs(3))
        );
        assert_eq!(
            policy.next_delay(1, asked, Some(Duration::from_secs(10))),
            Some(Duration::from_secs(3))
        );
        assert_eq!(
            policy.next_delay(1, asked, Some(Duration::from_secs(3))),
            None
        );

        let ignoring = RetryPolicy {
            respect_retry_after: false,
            ..no_jitter()
        };
        assert_eq!(
            ignoring.next_delay(1, asked, None),
            Some(Duration::from_millis(500))
        );
    }

    #[test]
    fn parses_retry_after_headers() {
        let parse = |pairs: &[(&'static str, &str)]| {
            let mut headers = HeaderMap::new();
            for (name, value) in pairs {
                headers.insert(*name, HeaderValue::from_str(value).unwrap());
            }
            retry_after(&headers)
        };
        assert_eq!(
            parse(&[("retry-after-ms", "250")]),
            Some(Duration::from_millis(250))
        );
        assert_eq!(parse(&[("retry-after", "2")]), Some(Duration::from_secs(2)));
        assert_eq!(
            parse(&[("retry-after", "0.5")]),
            Some(Duration::from_millis(500))
        );
        assert_eq!(
            parse(&[("retry-after-ms", "100"), ("retry-after", "9")]),
            Some(Duration::from_millis(100))
        );
        assert_eq!(
            parse(&[("retry-after-ms", "-1"), ("retry-after", "1")]),
            Some(Duration::from_secs(1))
        );
        assert_eq!(parse(&[("retry-after", "soon")]), None);
        assert_eq!(parse(&[]), None);

        let past = httpdate::fmt_http_date(SystemTime::now() - Duration::from_secs(60));
        assert_eq!(parse(&[("retry-after", &past)]), Some(Duration::ZERO));
        let future = httpdate::fmt_http_date(SystemTime::now() + Duration::from_secs(120));
        let wait = parse(&[("retry-after", &future)]).unwrap();
        assert!(wait > Duration::from_secs(100) && wait <= Duration::from_secs(120));
    }
}
