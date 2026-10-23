//! Asks the person at this computer before a one-time-password link starts.
//!
//! Phone scammers talk people into installing a remote-access app and reading
//! out its code. On Slipstream Public, and on stations that ask for it, a
//! link that used the one-time password therefore waits until someone at the
//! computer presses Allow. The unattended password never asks: its owner set
//! it up for computers nobody sits at.
use crate::{ClientEvent, Inner};
use serde::{Deserialize, Serialize};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::{oneshot, watch};

/// How long the person at the computer has to answer.
pub const ANSWER_WAIT: Duration = Duration::from_secs(60);
/// How long to wait for a window that can ask (the Windows service starts one
/// in the signed-in user's session) before declining.
const RECEIVER_WAIT: Duration = Duration::from_secs(if cfg!(test) { 1 } else { 10 });

pub const WAITING: &str = "Waiting for the person at the remote computer to allow this link…";
pub const DECLINED: &str = "The person at the remote computer declined the link";
pub const UNANSWERED: &str = "Nobody allowed the link on the remote computer in time";
pub const NOBODY: &str =
    "Nobody is at the remote computer to allow this link. Use its unattended password for unattended access";
pub const BUSY: &str = "Another link is already waiting for approval on that computer — try again shortly";

/// A link waiting for Allow or Decline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkRequest {
    pub id: u64,
    /// The name the viewer chose for itself. Not verified.
    pub peer: String,
    /// The link came through Slipstream Public rather than a private station.
    pub public: bool,
    /// Unix seconds when the request is declined automatically.
    pub expires: u64,
}

pub struct Approvals {
    pending: watch::Sender<Option<LinkRequest>>,
    answer: Mutex<Option<(u64, oneshot::Sender<bool>)>>,
    receivers: Arc<AtomicUsize>,
    slot: tokio::sync::Semaphore,
}

impl Default for Approvals {
    fn default() -> Self {
        Self {
            pending: watch::channel(None).0,
            answer: Mutex::new(None),
            receivers: Arc::default(),
            slot: tokio::sync::Semaphore::new(1),
        }
    }
}

/// Held by every window able to show the question. Without one, links that
/// need approval are declined straight away.
pub struct ReceiverGuard(Arc<AtomicUsize>);
impl Drop for ReceiverGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Approvals {
    pub fn receiver(&self) -> ReceiverGuard {
        self.receivers.fetch_add(1, Ordering::AcqRel);
        ReceiverGuard(self.receivers.clone())
    }
    pub fn pending(&self) -> Option<LinkRequest> {
        self.pending.borrow().clone()
    }
    pub fn answer(&self, id: u64, accepted: bool) {
        let mut answer = self.answer.lock().unwrap();
        if answer.as_ref().is_some_and(|(pending, _)| *pending == id)
            && let Some((_, reply)) = answer.take()
        {
            let _ = reply.send(accepted);
        }
    }
    fn has_receiver(&self) -> bool {
        self.receivers.load(Ordering::Acquire) > 0
    }
}

/// Asks, and returns the reason to give the viewer if the link may not go on.
/// `gone` resolves when the viewer hangs up while waiting.
pub(crate) async fn ask(
    inner: &Inner,
    peer: &str,
    public: bool,
    gone: impl std::future::Future<Output = ()>,
) -> Result<(), &'static str> {
    let approvals = &inner.approvals;
    let Ok(_slot) = approvals.slot.try_acquire() else {
        return Err(BUSY);
    };
    let id = rand::random::<u64>();
    let (tx, rx) = oneshot::channel();
    *approvals.answer.lock().unwrap() = Some((id, tx));
    let expires = now() + ANSWER_WAIT.as_secs();
    approvals.pending.send_replace(Some(LinkRequest { id, peer: peer.into(), public, expires }));
    inner.emit(ClientEvent::LinkApproval);
    let decision = async {
        if !approvals.has_receiver() {
            let appeared = tokio::time::timeout(RECEIVER_WAIT, async {
                while !approvals.has_receiver() {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            })
            .await;
            if appeared.is_err() {
                return Err(NOBODY);
            }
        }
        tokio::select! {
            answer = rx => if answer.unwrap_or(false) { Ok(()) } else { Err(DECLINED) },
            _ = tokio::time::sleep(ANSWER_WAIT) => Err(UNANSWERED),
            _ = async { while approvals.has_receiver() { tokio::time::sleep(Duration::from_millis(250)).await; } } => Err(NOBODY),
        }
    };
    let result = tokio::select! {
        r = decision => r,
        _ = gone => Err(DECLINED),
    };
    approvals.answer.lock().unwrap().take();
    approvals.pending.send_replace(None);
    inner.emit(ClientEvent::LinkApproval);
    result
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}
