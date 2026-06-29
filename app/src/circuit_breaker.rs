use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakerState {
    Closed,
    Open,
    HalfOpen,
}

impl BreakerState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Open => "open",
            Self::HalfOpen => "half_open",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CircuitBreakerSnapshot {
    pub state: BreakerState,
    pub failure_count: u32,
    pub last_success_unix_secs: Option<u64>,
}

#[derive(Debug)]
struct CircuitBreakerInner {
    state: BreakerState,
    failure_count: u32,
    opened_at: Option<Instant>,
    last_success_unix_secs: Option<u64>,
}

#[derive(Debug)]
pub struct CircuitBreaker {
    failure_threshold: u32,
    cooldown: Duration,
    inner: Mutex<CircuitBreakerInner>,
}

impl CircuitBreaker {
    pub fn new(failure_threshold: u32, cooldown: Duration) -> Self {
        Self {
            failure_threshold,
            cooldown,
            inner: Mutex::new(CircuitBreakerInner {
                state: BreakerState::Closed,
                failure_count: 0,
                opened_at: None,
                last_success_unix_secs: None,
            }),
        }
    }

    pub async fn before_request(&self) -> BreakerState {
        let mut inner = self.inner.lock().await;

        if inner.state == BreakerState::Open
            && inner
                .opened_at
                .is_some_and(|opened_at| opened_at.elapsed() >= self.cooldown)
        {
            inner.state = BreakerState::HalfOpen;
        }

        inner.state
    }

    pub async fn record_success(&self) {
        let mut inner = self.inner.lock().await;

        inner.state = BreakerState::Closed;
        inner.failure_count = 0;
        inner.opened_at = None;
        inner.last_success_unix_secs = Some(now_unix_secs());
    }

    pub async fn record_failure(&self) {
        let mut inner = self.inner.lock().await;

        inner.failure_count += 1;
        if inner.state == BreakerState::HalfOpen || inner.failure_count >= self.failure_threshold {
            inner.state = BreakerState::Open;
            inner.opened_at = Some(Instant::now());
        }
    }

    pub async fn snapshot(&self) -> CircuitBreakerSnapshot {
        let state = self.before_request().await;
        let inner = self.inner.lock().await;

        CircuitBreakerSnapshot {
            state,
            failure_count: inner.failure_count,
            last_success_unix_secs: inner.last_success_unix_secs,
        }
    }
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::sleep;

    #[tokio::test]
    async fn repeated_failures_open_the_breaker() {
        let breaker = CircuitBreaker::new(2, Duration::from_secs(30));

        assert_eq!(breaker.before_request().await, BreakerState::Closed);
        breaker.record_failure().await;
        assert_eq!(breaker.before_request().await, BreakerState::Closed);
        breaker.record_failure().await;
        assert_eq!(breaker.before_request().await, BreakerState::Open);
    }

    #[tokio::test]
    async fn open_breaker_moves_half_open_after_cooldown() {
        let breaker = CircuitBreaker::new(1, Duration::from_millis(5));

        breaker.record_failure().await;
        assert_eq!(breaker.before_request().await, BreakerState::Open);

        sleep(Duration::from_millis(10)).await;
        assert_eq!(breaker.before_request().await, BreakerState::HalfOpen);
    }

    #[tokio::test]
    async fn success_closes_half_open_breaker() {
        let breaker = CircuitBreaker::new(1, Duration::from_millis(1));

        breaker.record_failure().await;
        sleep(Duration::from_millis(5)).await;
        assert_eq!(breaker.before_request().await, BreakerState::HalfOpen);

        breaker.record_success().await;
        let snapshot = breaker.snapshot().await;
        assert_eq!(snapshot.state, BreakerState::Closed);
        assert_eq!(snapshot.failure_count, 0);
        assert!(snapshot.last_success_unix_secs.is_some());
    }

    #[tokio::test]
    async fn failed_half_open_request_reopens_breaker() {
        let breaker = CircuitBreaker::new(1, Duration::from_millis(1));

        breaker.record_failure().await;
        sleep(Duration::from_millis(5)).await;
        assert_eq!(breaker.before_request().await, BreakerState::HalfOpen);

        breaker.record_failure().await;
        assert_eq!(breaker.before_request().await, BreakerState::Open);
    }
}
