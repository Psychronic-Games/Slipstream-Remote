//! Bounded recovery of an established link, always using a fresh station lookup.
use crate::{StationRef, Client, ViewerEvent, ViewerSession};
use anyhow::{Context, Result, bail};
use slipstream_core::directory::DeviceId;
use std::{future::Future, time::Duration};
use tokio::time::Instant;

#[derive(Clone, Copy)]
struct Policy {
    window: Duration,
    attempt: Duration,
    initial: Duration,
    maximum: Duration,
}
const POLICY: Policy = Policy {
    window: Duration::from_secs(120),
    attempt: Duration::from_secs(20),
    initial: Duration::from_secs(1),
    maximum: Duration::from_secs(8),
};

impl Client {
    /// Retry a lost link for two minutes. Dropping this future cancels the
    /// current attempt and all future retries. Rejected passwords never retry.
    pub async fn reconnect(
        &self,
        id: DeviceId,
        password: &str,
        via: &StationRef,
        on_progress: impl Fn(String) + Send + Sync + 'static,
    ) -> Result<(
        ViewerSession,
        tokio::sync::mpsc::UnboundedReceiver<ViewerEvent>,
    )> {
        let progress = std::sync::Arc::new(on_progress);
        retry(
            POLICY,
            || {
                let progress = progress.clone();
                async move {
                    let notify = move |text| progress(text);
                    self.connect_on(via, id, password, notify).await
                }
            },
            |number, delay| {
                progress(format!(
                    "Waiting for the remote host · attempt {number} in {} s · Cancel to stop",
                    delay.as_secs()
                ))
            },
        )
        .await
    }
}

async fn retry<T, F, Fut>(
    policy: Policy,
    mut connect: F,
    notify: impl Fn(u32, Duration),
) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let deadline = Instant::now() + policy.window;
    let mut delay = policy.initial;
    let mut number = 0;
    let mut last = None;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining <= delay {
            if let Some(error) = last {
                return Err(error).context("Automatic reconnect stopped after two minutes; try Reconnect when the host is ready");
            }
            bail!("Automatic reconnect timed out");
        }
        number += 1;
        notify(number, delay);
        tokio::time::sleep(delay).await;
        let remaining = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(policy.attempt.min(remaining), connect()).await {
            Ok(Ok(value)) => return Ok(value),
            Ok(Err(error)) if error.is::<crate::viewer::AuthRejected>() => return Err(error),
            Ok(Err(error)) => last = Some(error),
            Err(_) => {
                last = Some(anyhow::anyhow!(
                    "The host did not answer the connection attempt"
                ))
            }
        }
        delay = (delay * 2).min(policy.maximum);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    };
    fn policy() -> Policy {
        Policy {
            window: Duration::from_millis(150),
            attempt: Duration::from_millis(25),
            initial: Duration::from_millis(1),
            maximum: Duration::from_millis(5),
        }
    }

    #[tokio::test]
    async fn host_returns_and_auth_rejection_is_not_retried() {
        let mut calls = 0;
        let result = retry(
            policy(),
            || {
                calls += 1;
                let n = calls;
                async move {
                    if n < 3 {
                        bail!("offline")
                    }
                    Ok(42)
                }
            },
            |_, _| {},
        )
        .await
        .unwrap();
        assert_eq!((result, calls), (42, 3));
        let mut calls = 0;
        let error = retry(
            policy(),
            || {
                calls += 1;
                async {
                    Err::<(), _>(crate::viewer::AuthRejected("password changed".into()).into())
                }
            },
            |_, _| {},
        )
        .await
        .unwrap_err();
        assert!(error.is::<crate::viewer::AuthRejected>());
        assert_eq!(calls, 1);
    }

    #[tokio::test]
    async fn hung_attempt_is_bounded_and_cancelling_stops_retries() {
        let start = Instant::now();
        let result = retry(policy(), std::future::pending::<Result<()>>, |_, _| {}).await;
        assert!(result.is_err());
        assert!(start.elapsed() < Duration::from_secs(1));
        let calls = Arc::new(AtomicU32::new(0));
        let count = calls.clone();
        let task = tokio::spawn(async move {
            retry(
                policy(),
                || {
                    count.fetch_add(1, Ordering::SeqCst);
                    async { bail!("offline") }
                },
                |_, _| {},
            )
            .await as Result<()>
        });
        while calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        task.abort();
        let _ = task.await;
        let stopped = calls.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(calls.load(Ordering::SeqCst), stopped);
    }
}
