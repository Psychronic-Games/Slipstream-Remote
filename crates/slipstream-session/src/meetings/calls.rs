//! Incoming call offers never open desktop capture or grant remote input.
use super::wire;
use crate::{Client, ClientEvent, Inner};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use slipstream_core::{
    config::ServerSettings,
    directory::{DeviceId, DirectoryClient, Lookup},
    meeting::*,
};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{oneshot, watch};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IncomingCall {
    pub id: u64,
    pub name: String,
    pub invitation: Invitation,
    pub expires: u64,
}
pub struct Broker {
    incoming: watch::Sender<Option<IncomingCall>>,
    answer: Mutex<Option<(u64, oneshot::Sender<bool>)>>,
    receivers: Arc<AtomicUsize>,
    last: Mutex<Option<Instant>>,
    slot: tokio::sync::Semaphore,
}
impl Default for Broker {
    fn default() -> Self {
        Self {
            incoming: watch::channel(None).0,
            answer: Mutex::new(None),
            receivers: Arc::default(),
            last: Mutex::new(None),
            slot: tokio::sync::Semaphore::new(1),
        }
    }
}
pub struct ReceiverGuard(Arc<AtomicUsize>);
impl Drop for ReceiverGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}
impl Broker {
    pub fn receiver(&self) -> ReceiverGuard {
        self.receivers.fetch_add(1, Ordering::AcqRel);
        ReceiverGuard(self.receivers.clone())
    }
    pub fn incoming(&self) -> Option<IncomingCall> {
        self.incoming.borrow().clone()
    }
    pub fn answer(&self, id: u64, accepted: bool) {
        let mut answer = self.answer.lock().unwrap();
        if answer.as_ref().is_some_and(|(pending, _)| *pending == id)
            && let Some((_, reply)) = answer.take()
        {
            let _ = reply.send(accepted);
        }
    }
}
pub(crate) async fn receive(inner: Arc<Inner>, conn: iroh::endpoint::Connection) -> Result<()> {
    let broker = &inner.calls;
    let _permit = broker
        .slot
        .try_acquire()
        .context("Another call is already ringing")?;
    let (mut send, mut recv) =
        tokio::time::timeout(Duration::from_secs(5), conn.accept_bi()).await??;
    ensure!(
        matches!(
            tokio::time::timeout(
                Duration::from_secs(5),
                wire::read::<CallMessage>(&mut recv, MAX_CONTROL)
            )
            .await??,
            CallMessage::Hello
        ),
        "Expected call hello"
    );
    if broker.receivers.load(Ordering::Acquire) == 0
        || inner.settings.lock().unwrap().app.block_meeting_calls
    {
        wire::write(
            &mut send,
            &CallMessage::Declined("Nobody is available to answer on that device".into()),
            MAX_CONTROL,
        )
        .await?;
        send.finish().ok();
        return Ok(());
    }
    {
        let last = broker.last.lock().unwrap();
        ensure!(
            last.is_none_or(|last| last.elapsed() > Duration::from_secs(5)),
            "Wait a few seconds before calling again"
        );
    }
    let nonce = rand::random();
    wire::write(&mut send, &CallMessage::Challenge(nonce), MAX_CONTROL).await?;
    let CallMessage::Offer {
        name,
        invitation,
        proof,
    } = tokio::time::timeout(Duration::from_secs(5), wire::read(&mut recv, MAX_CONTROL)).await??
    else {
        bail!("Expected call offer");
    };
    invitation.validate()?;
    let name = clean_name(&name)?;
    let key = inner.settings.lock().unwrap().server.key.clone();
    ensure!(
        equal(
            &proof,
            &call_proof(
                key.as_deref(),
                &nonce,
                conn.remote_id(),
                inner.endpoint.id(),
                &invitation
            )?
        ),
        "Caller is not authorized for this station"
    );
    *broker.last.lock().unwrap() = Some(Instant::now());
    let id = rand::random::<u64>();
    let call = IncomingCall {
        id,
        name,
        invitation,
        expires: now() + 35,
    };
    let (tx, rx) = oneshot::channel();
    *broker.answer.lock().unwrap() = Some((id, tx));
    broker.incoming.send_replace(Some(call));
    inner.emit(ClientEvent::CallChanged);
    #[cfg(target_os = "android")]
    {
        let call = broker.incoming();
        if let Some(call) = call {
            slipstream_platform::android::notify_call(Some(&call.name), call.id);
        }
    }
    let ring = wire::write(&mut send, &CallMessage::Ringing, MAX_CONTROL).await;
    let accepted = if ring.is_ok() {
        tokio::select! {
            result=rx=>result.unwrap_or(false),
            _=tokio::time::sleep(Duration::from_secs(35))=>false,
            _=wire::read::<CallMessage>(&mut recv,MAX_CONTROL)=>false,
            _=conn.closed()=>false,
            _=async{while broker.receivers.load(Ordering::Acquire)>0{tokio::time::sleep(Duration::from_millis(250)).await;}}=>false,
        }
    } else {
        false
    };
    broker.answer.lock().unwrap().take();
    broker.incoming.send_replace(None);
    inner.emit(ClientEvent::CallChanged);
    #[cfg(target_os = "android")]
    slipstream_platform::android::notify_call(None, 0);
    let message = if accepted {
        CallMessage::Answered
    } else {
        CallMessage::Declined("Call declined or unanswered".into())
    };
    let _ = wire::write(&mut send, &message, MAX_CONTROL).await;
    send.finish().ok();
    let _ = tokio::time::timeout(Duration::from_millis(500), send.stopped()).await;
    conn.close(0u32.into(), b"call completed");
    Ok(())
}
#[derive(Clone, Debug)]
pub enum Progress {
    Calling,
    Ringing,
    Answered,
    Ended(String),
}
pub struct Outgoing {
    pub state: watch::Receiver<Progress>,
    cancel: Option<oneshot::Sender<()>>,
}
impl Drop for Outgoing {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
    }
}
impl Outgoing {
    pub fn start(
        client: Client,
        server: ServerSettings,
        id: DeviceId,
        invitation: Invitation,
    ) -> Self {
        let (tx, state) = watch::channel(Progress::Calling);
        let (cancel, mut cancelled) = oneshot::channel();
        tokio::spawn(async move {
            let work = async {
                let url = server
                    .url
                    .as_deref()
                    .context("Configure a station before calling a device")?;
                let directory = DirectoryClient::new(url, server.key.clone())?;
                let Lookup::Online { addr, .. } = directory.lookup(id).await? else {
                    bail!("That device is offline");
                };
                slipstream_core::net::add_relays(&client.inner.endpoint, &server.relays).await;
                let conn = tokio::time::timeout(
                    Duration::from_secs(15),
                    client.inner.endpoint.connect(addr, CALL_ALPN),
                )
                .await
                .context("That device did not answer")?
                .context("Cannot call that device; it may need an update")?;
                struct Close(iroh::endpoint::Connection);
                impl Drop for Close {
                    fn drop(&mut self) {
                        self.0.close(0u32.into(), b"call completed");
                    }
                }
                let _close = Close(conn.clone());
                let (mut send, mut recv) = conn.open_bi().await?;
                wire::write(&mut send, &CallMessage::Hello, MAX_CONTROL).await?;
                let message: CallMessage = tokio::time::timeout(
                    Duration::from_secs(5),
                    wire::read(&mut recv, MAX_CONTROL),
                )
                .await??;
                let nonce = match message {
                    CallMessage::Challenge(nonce) => nonce,
                    CallMessage::Declined(reason) => bail!(reason),
                    _ => bail!("Unexpected call reply"),
                };
                let proof = call_proof(
                    server.key.as_deref(),
                    &nonce,
                    client.inner.endpoint.id(),
                    conn.remote_id(),
                    &invitation,
                )?;
                wire::write(
                    &mut send,
                    &CallMessage::Offer {
                        name: client.settings().device_name,
                        invitation,
                        proof,
                    },
                    MAX_CONTROL,
                )
                .await?;
                let result = async {
                    loop {
                        match wire::read::<CallMessage>(&mut recv, MAX_CONTROL).await? {
                            CallMessage::Ringing => {
                                tx.send_replace(Progress::Ringing);
                            }
                            CallMessage::Answered => return anyhow::Ok(()),
                            CallMessage::Declined(reason) => bail!(reason),
                            _ => bail!("Unexpected call reply"),
                        }
                    }
                };
                let result = tokio::time::timeout(Duration::from_secs(40), result)
                    .await
                    .context("Call was not answered")?;
                conn.close(0u32.into(), b"call completed");
                result
            };
            let result = tokio::select! {
                result=tokio::time::timeout(Duration::from_secs(65),work)=>result,
                _=&mut cancelled=>Ok(Err(anyhow::anyhow!("Call cancelled"))),
            };
            match result {
                Ok(Ok(())) => {
                    tx.send_replace(Progress::Answered);
                }
                Ok(Err(e)) => {
                    tx.send_replace(Progress::Ended(format!("{e:#}")));
                }
                Err(_) => {
                    tx.send_replace(Progress::Ended("Call timed out".into()));
                }
            }
        });
        Self {
            state,
            cancel: Some(cancel),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn authenticated_calls_answer_decline_cancel_and_never_open_desktop() {
        let dir = tempfile::tempdir().unwrap();
        let store = slipstream_core::config::Store::open(dir.path()).unwrap();
        let mut settings = store.load_settings();
        settings.server.use_public_relays = false;
        settings.server.key = Some("call-station-key".into());
        store.save_settings(&settings).unwrap();
        let (callee, _events) = Client::start(
            store,
            crate::ClientOptions {
                accept_sessions: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let caller = slipstream_core::net::bind_client(
            iroh::SecretKey::generate(),
            &ServerSettings {
                use_public_relays: false,
                ..Default::default()
            },
            false,
        )
        .await
        .unwrap();
        let broker = callee.call_broker();
        let receiver = broker.receiver();
        let invite = Invitation {
            version: VERSION,
            address: caller.addr(),
            token: random_secret(),
            title: "Support".into(),
            expires: now() + 3600,
            public_relays: false,
        };
        for action in 0..3 {
            *broker.last.lock().unwrap() = None;
            let conn = caller
                .connect(callee.endpoint_addr(), CALL_ALPN)
                .await
                .unwrap();
            let (mut send, mut recv) = conn.open_bi().await.unwrap();
            wire::write(&mut send, &CallMessage::Hello, MAX_CONTROL)
                .await
                .unwrap();
            let CallMessage::Challenge(nonce) = wire::read(&mut recv, MAX_CONTROL).await.unwrap()
            else {
                panic!("Expected challenge");
            };
            let proof = call_proof(
                Some("call-station-key"),
                &nonce,
                caller.id(),
                callee.endpoint_id(),
                &invite,
            )
            .unwrap();
            wire::write(
                &mut send,
                &CallMessage::Offer {
                    name: "Technician".into(),
                    invitation: invite.clone(),
                    proof,
                },
                MAX_CONTROL,
            )
            .await
            .unwrap();
            assert!(matches!(
                wire::read::<CallMessage>(&mut recv, MAX_CONTROL)
                    .await
                    .unwrap(),
                CallMessage::Ringing
            ));
            let call = broker.incoming().unwrap();
            assert!(callee.inner.backend.lock().await.is_none());
            assert!(callee.inner.host_sessions.lock().unwrap().is_empty());
            if action < 2 {
                broker.answer(call.id, action == 1);
                let result = wire::read::<CallMessage>(&mut recv, MAX_CONTROL)
                    .await
                    .unwrap();
                assert!(matches!(
                    (action, result),
                    (0, CallMessage::Declined(_)) | (1, CallMessage::Answered)
                ));
            } else {
                wire::write(&mut send, &CallMessage::Cancel, MAX_CONTROL)
                    .await
                    .unwrap();
            }
            tokio::time::timeout(Duration::from_secs(2), async {
                while broker.incoming().is_some() {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
            conn.close(0u32.into(), b"test completed");
        }
        *broker.last.lock().unwrap() = None;
        let conn = caller
            .connect(callee.endpoint_addr(), CALL_ALPN)
            .await
            .unwrap();
        let (mut send, mut recv) = conn.open_bi().await.unwrap();
        wire::write(&mut send, &CallMessage::Hello, MAX_CONTROL)
            .await
            .unwrap();
        let CallMessage::Challenge(nonce) = wire::read(&mut recv, MAX_CONTROL).await.unwrap()
        else {
            panic!("Expected challenge");
        };
        let proof = call_proof(
            Some("wrong-station"),
            &nonce,
            caller.id(),
            callee.endpoint_id(),
            &invite,
        )
        .unwrap();
        wire::write(
            &mut send,
            &CallMessage::Offer {
                name: "Untrusted".into(),
                invitation: invite,
                proof,
            },
            MAX_CONTROL,
        )
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(broker.incoming().is_none());
        conn.close(0u32.into(), b"test completed");
        drop(receiver);
        caller.close().await;
        callee.shutdown().await;
    }
}
