//! Relay diagnostics. Never record request URLs, headers, access keys or passwords.
//! The upstream client actor reports its actual transport error inside a span;
//! retain that cause and emit one contextual close record after the proxy ends.
use crate::db::Db;
use serde::Deserialize;
use std::{
    collections::HashMap,
    fmt,
    io::{IsTerminal, Read},
    net::IpAddr,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};
use tracing::{
    Event, Metadata, Subscriber,
    field::{Field, Visit},
    span::{Attributes, Id},
};
use tracing_subscriber::{
    EnvFilter, Layer,
    layer::{Context, Filter, SubscriberExt},
    registry::LookupSpan,
    util::SubscriberInitExt,
};

pub const LOG_FILTER: &str =
    "warn,slipstream_server=info,slipstream_relay=info,iroh_relay::server::client=info";
pub const TUNNEL_HEADER: &str = "x-slipstream-relay-tunnel";

#[derive(Clone, Debug)]
pub struct Identity {
    pub endpoint: String,
    pub connection: String,
    pub protocol: String,
    pub device_id: Option<u32>,
    pub name: Option<String>,
}
#[derive(Clone, Debug)]
enum CloseCause {
    StreamEnded,
    Failure(String),
}
struct Active {
    identity: Identity,
    tunnel: Option<u64>,
    started: Instant,
    cause: Option<CloseCause>,
}
struct Tunnel {
    started: Instant,
    peer: Option<IpAddr>,
    identity: Option<Identity>,
    cause: Option<CloseCause>,
    denied: bool,
    handler_closed: bool,
    result: Option<Result<(u64, u64), String>>,
    duration_ms: Option<u64>,
}
#[derive(Default)]
struct State {
    active: HashMap<String, Active>,
    tunnels: HashMap<u64, Tunnel>,
}
struct Resolver {
    db: Arc<Db>,
    station: Option<PathBuf>,
}
#[derive(Default)]
pub struct RelayLog {
    state: Mutex<State>,
    next: AtomicU64,
    resolver: OnceLock<Resolver>,
}
impl fmt::Debug for RelayLog {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RelayLog")
            .field("active", &self.active_count())
            .finish_non_exhaustive()
    }
}
impl RelayLog {
    pub fn configure(&self, db: Arc<Db>, station: Option<PathBuf>, private: bool) {
        if private {
            let _ = self.resolver.set(Resolver { db, station });
        }
    }
    pub fn active_count(&self) -> usize {
        self.state.lock().unwrap().active.len()
    }
    pub fn begin(&self, peer: Option<IpAddr>) -> u64 {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        self.state.lock().unwrap().tunnels.insert(
            id,
            Tunnel {
                started: Instant::now(),
                peer,
                identity: None,
                cause: None,
                denied: false,
                handler_closed: false,
                result: None,
                duration_ms: None,
            },
        );
        id
    }
    pub async fn identity(self: &Arc<Self>, endpoint: String) -> Option<(u32, String)> {
        self.resolver.get()?;
        let this = self.clone();
        let lookup = tokio::task::spawn_blocking(move || {
            let resolver = this.resolver.get()?;
            if let Some(station) = &resolver.station {
                // Endpoint ids are authenticated public keys, not arbitrary paths.
                if endpoint.len() != 64 || !endpoint.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return None;
                }
                let record: Device = read_json(
                    &station.join("devices").join(format!("{endpoint}.json")),
                    16 * 1024,
                )?;
                let alias = read_json::<Shared>(&station.join("shared.json"), 4 * 1024 * 1024)
                    .and_then(|s| s.devices.into_iter().find(|d| d.id == record.id))
                    .map(|d| d.name)
                    .filter(|n| !n.is_empty());
                Some((record.id, alias.unwrap_or(record.name)))
            } else {
                resolver.db.describe_endpoint(&endpoint).ok().flatten()
            }
        });
        // Optional labels must not hold up the relay's authentication path.
        tokio::time::timeout(std::time::Duration::from_millis(250), lookup)
            .await
            .ok()?
            .ok()
            .flatten()
    }
    pub fn admit(
        &self,
        identity: Identity,
        tunnel: Option<u64>,
        maximum: usize,
    ) -> Result<(), usize> {
        let mut state = self.state.lock().unwrap();
        if maximum > 0 && state.active.len() >= maximum {
            return Err(state.active.len());
        }
        let tunnel = tunnel.filter(|id| state.tunnels.contains_key(id));
        let peer = tunnel
            .and_then(|id| state.tunnels.get_mut(&id))
            .and_then(|t| {
                t.identity = Some(identity.clone());
                t.peer
            });
        let endpoint_connections = 1 + state
            .active
            .values()
            .filter(|a| a.identity.endpoint == identity.endpoint)
            .count();
        state.active.insert(
            identity.connection.clone(),
            Active {
                identity: identity.clone(),
                tunnel,
                started: Instant::now(),
                cause: None,
            },
        );
        let active = state.active.len();
        drop(state);
        tracing::info!(target: "slipstream_relay", event="relay_client_connected", connection=%identity.connection, tunnel, device_id=identity.device_id, device_name=identity.name.as_deref(), endpoint=%identity.endpoint, peer=peer.map(|ip| ip.to_string()).as_deref(), protocol=%identity.protocol, active_connections=active, endpoint_connections, "Relay client connected");
        Ok(())
    }
    pub fn deny(
        &self,
        endpoint: &str,
        connection: &str,
        tunnel: Option<u64>,
        reason: &'static str,
        maximum: usize,
    ) {
        let mut state = self.state.lock().unwrap();
        let peer = tunnel
            .and_then(|id| state.tunnels.get_mut(&id))
            .and_then(|t| {
                t.denied = true;
                t.peer
            });
        let active = state.active.len();
        drop(state);
        tracing::warn!(target: "slipstream_relay", event="relay_client_denied", %connection, tunnel, %endpoint, peer=peer.map(|ip| ip.to_string()).as_deref(), reason, active_connections=active, max_connections=maximum, "Relay connection rejected");
    }
    pub fn disconnected(&self, connection: &str) {
        let mut state = self.state.lock().unwrap();
        let Some(active) = state.active.remove(connection) else {
            return;
        };
        let remaining = state.active.len();
        if let Some(id) = active.tunnel
            && let Some(tunnel) = state.tunnels.get_mut(&id)
        {
            tunnel.cause = active.cause;
            tunnel.handler_closed = true;
            let complete = tunnel.result.is_some();
            let finished = complete.then(|| state.tunnels.remove(&id).unwrap());
            drop(state);
            if let Some(tunnel) = finished {
                finished_record(id, tunnel, remaining);
            }
            return;
        }
        drop(state);
        close_record(
            &active.identity,
            active.tunnel,
            None,
            active.started.elapsed().as_millis() as u64,
            active.cause,
            None,
            None,
            remaining,
        );
    }
    fn observe_actor(&self, connection: &str, message: &str) -> bool {
        let Some(error) = message
            .strip_prefix("actor errored ")
            .and_then(|m| m.strip_suffix(", exiting"))
        else {
            return false;
        };
        let mut state = self.state.lock().unwrap();
        let Some(active) = state.active.get_mut(connection) else {
            return false;
        };
        active.cause = Some(if error == "Stream terminated" {
            CloseCause::StreamEnded
        } else {
            CloseCause::Failure(error.chars().take(2048).collect())
        });
        true
    }
    pub fn request_failed(
        &self,
        tunnel: Option<u64>,
        path: &str,
        reason: &'static str,
        error: Option<String>,
        status: Option<u16>,
    ) {
        let snapshot = tunnel.and_then(|id| self.state.lock().unwrap().tunnels.remove(&id));
        let peer = snapshot.and_then(|t| t.peer);
        tracing::warn!(target: "slipstream_relay", event="relay_request_failed", tunnel, path, peer=peer.map(|ip| ip.to_string()).as_deref(), reason, error=error.as_deref(), status, "Relay request failed");
    }
    pub async fn finish(self: &Arc<Self>, tunnel: u64, result: std::io::Result<(u64, u64)>) {
        let (identity, duration, peer) = {
            let mut state = self.state.lock().unwrap();
            let Some(snapshot) = state.tunnels.get(&tunnel) else {
                return;
            };
            if snapshot.denied {
                state.tunnels.remove(&tunnel);
                return;
            }
            (
                snapshot.identity.clone(),
                snapshot.started.elapsed().as_millis() as u64,
                snapshot.peer,
            )
        };
        let Some(mut identity) = identity else {
            self.state.lock().unwrap().tunnels.remove(&tunnel);
            tracing::warn!(target: "slipstream_relay", event="relay_handshake_incomplete", tunnel, peer=peer.map(|ip| ip.to_string()).as_deref(), duration_ms=duration, reason="relay_authentication_not_completed", error=result.err().map(|e| e.to_string()).as_deref(), "Relay transport ended before client authentication completed");
            return;
        };
        // A freshly installed host may register after its first relay handshake.
        if identity.device_id.is_none()
            && let Some((id, name)) = self.identity(identity.endpoint.clone()).await
        {
            identity.device_id = Some(id);
            identity.name = Some(name);
        }
        let mut state = self.state.lock().unwrap();
        let Some(snapshot) = state.tunnels.get_mut(&tunnel) else {
            return;
        };
        snapshot.identity = Some(identity);
        snapshot.duration_ms = Some(duration);
        snapshot.result = Some(result.map_err(|e| e.to_string()));
        // Wait for both the final proxy totals and the upstream handler's cause,
        // whichever arrives first. This also prevents duplicate close messages.
        let complete = snapshot.handler_closed;
        let finished = complete.then(|| state.tunnels.remove(&tunnel).unwrap());
        let remaining = state.active.len();
        drop(state);
        if let Some(snapshot) = finished {
            finished_record(tunnel, snapshot, remaining);
        }
    }
}

fn finished_record(id: u64, tunnel: Tunnel, remaining: usize) {
    let Some(identity) = tunnel.identity else {
        return;
    };
    let (counts, error) = match tunnel.result {
        Some(Ok(counts)) => (Some(counts), None),
        Some(Err(error)) => (None, Some(error)),
        None => return,
    };
    close_record(
        &identity,
        Some(id),
        tunnel.peer,
        tunnel.duration_ms.unwrap_or_default(),
        tunnel.cause,
        counts,
        error,
        remaining,
    );
}

#[derive(Deserialize)]
struct Device {
    id: u32,
    #[serde(default)]
    name: String,
}
#[derive(Deserialize)]
struct Shared {
    devices: Vec<Device>,
}
fn read_json<T: serde::de::DeserializeOwned>(path: &Path, limit: u64) -> Option<T> {
    let file = std::fs::File::open(path).ok()?;
    if file.metadata().ok()?.len() > limit {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > limit {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}
#[allow(clippy::too_many_arguments)]
fn close_record(
    identity: &Identity,
    tunnel: Option<u64>,
    peer: Option<IpAddr>,
    duration: u64,
    cause: Option<CloseCause>,
    counts: Option<(u64, u64)>,
    transport_error: Option<String>,
    remaining: usize,
) {
    let (reason, error) = match cause {
        Some(CloseCause::Failure(e)) => ("relay_transport_error", Some(e)),
        _ if transport_error.is_some() => ("proxy_transport_error", transport_error),
        Some(CloseCause::StreamEnded) => ("client_stream_ended", None),
        None => ("close_reason_not_supplied", None),
    };
    let received = counts.map(|c| c.0);
    let sent = counts.map(|c| c.1);
    if let Some(error) = error {
        tracing::warn!(target: "slipstream_relay", event="relay_client_disconnected", connection=%identity.connection, tunnel, device_id=identity.device_id, device_name=identity.name.as_deref(), endpoint=%identity.endpoint, peer=peer.map(|ip| ip.to_string()).as_deref(), duration_ms=duration, bytes_from_client=received, bytes_to_client=sent, active_connections=remaining, reason, %error, "Relay client disconnected after transport failure");
    } else {
        tracing::info!(target: "slipstream_relay", event="relay_client_disconnected", connection=%identity.connection, tunnel, device_id=identity.device_id, device_name=identity.name.as_deref(), endpoint=%identity.endpoint, peer=peer.map(|ip| ip.to_string()).as_deref(), duration_ms=duration, bytes_from_client=received, bytes_to_client=sent, active_connections=remaining, reason, "Relay client disconnected");
    }
}

#[derive(Default)]
struct Fields {
    connection: Option<String>,
    message: Option<String>,
}
impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "connection_id" => self.connection = Some(value.to_string()),
            "message" => self.message = Some(value.to_string()),
            _ => {}
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.record_str(field, &format!("{value:?}"));
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.record_str(field, &value.to_string());
    }
}
struct ActorConnection(String);
pub struct ActorContext;
impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for ActorContext {
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        if attrs.metadata().target() != "iroh_relay::server::client"
            || attrs.metadata().name() != "client-connection-actor"
        {
            return;
        }
        let mut fields = Fields::default();
        attrs.record(&mut fields);
        if let (Some(connection), Some(span)) = (fields.connection, ctx.span(id)) {
            span.extensions_mut().insert(ActorConnection(connection));
        }
    }
}
pub struct RelayEvents(pub Arc<RelayLog>);
impl<S: Subscriber + for<'a> LookupSpan<'a>> Filter<S> for RelayEvents {
    fn enabled(&self, _: &Metadata<'_>, _: &Context<'_, S>) -> bool {
        true
    }
    fn event_enabled(&self, event: &Event<'_>, ctx: &Context<'_, S>) -> bool {
        if event.metadata().target() != "iroh_relay::server::client"
            || *event.metadata().level() != tracing::Level::WARN
        {
            return true;
        }
        let mut fields = Fields::default();
        event.record(&mut fields);
        let Some(message) = fields.message else {
            return true;
        };
        if let Some(scope) = ctx.event_scope(event) {
            for span in scope {
                if let Some(connection) = span.extensions().get::<ActorConnection>() {
                    // Only replace a warning when its precise connection is tracked.
                    return !self.0.observe_actor(&connection.0, &message);
                }
            }
        }
        true
    }
}
pub fn init(log: Arc<RelayLog>) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| LOG_FILTER.into());
    let ansi = std::io::stdout().is_terminal()
        && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty());
    tracing_subscriber::registry()
        .with(filter)
        .with(ActorContext)
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(ansi)
                .with_filter(RelayEvents(log)),
        )
        .init();
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write;
    #[derive(Clone, Default)]
    pub(crate) struct Capture(Arc<Mutex<Vec<u8>>>);
    impl Capture {
        pub(crate) fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }
    impl Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    pub(crate) fn subscriber(
        log: Arc<RelayLog>,
        capture: Capture,
    ) -> impl Subscriber + Send + Sync {
        tracing_subscriber::registry()
            .with(EnvFilter::new(LOG_FILTER))
            .with(ActorContext)
            .with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_writer(move || capture.clone())
                    .with_filter(RelayEvents(log)),
            )
    }
    fn identity(connection: &str) -> Identity {
        Identity {
            endpoint: "ab".repeat(32),
            connection: connection.into(),
            protocol: "V2".into(),
            device_id: Some(482913077),
            name: Some("Reception laptop".into()),
        }
    }
    #[tokio::test]
    async fn stream_end_becomes_one_identified_info_record_with_final_traffic() {
        let log = Arc::new(RelayLog::default());
        let output = Capture::default();
        let _guard = tracing::subscriber::set_default(subscriber(log.clone(), output.clone()));
        let tunnel = log.begin(Some("192.0.2.42".parse().unwrap()));
        log.admit(identity("17"), Some(tunnel), 1).unwrap();
        {
            let span = tracing::info_span!(target:"iroh_relay::server::client","client-connection-actor", connection_id=%17, remote_endpoint="abab");
            let _entered = span.enter();
            tracing::warn!(target:"iroh_relay::server::client","actor errored Stream terminated, exiting");
        }
        log.disconnected("17");
        log.finish(tunnel, Ok((1234, 5678))).await;
        let text = output.text();
        assert!(
            text.contains("Reception laptop")
                && text.contains("482913077")
                && text.contains("192.0.2.42")
        );
        assert!(
            text.contains("client_stream_ended")
                && text.contains("bytes_from_client=1234")
                && text.contains("bytes_to_client=5678")
        );
        assert_eq!(text.matches("relay_client_disconnected").count(), 1);
        assert!(
            !text.contains("WARN") && !text.contains("actor errored") && !text.contains('\x1b')
        );
        assert_eq!(log.active_count(), 0);
    }
    #[tokio::test]
    async fn transport_errors_keep_their_reason_and_warning_severity() {
        let log = Arc::new(RelayLog::default());
        let output = Capture::default();
        let _guard = tracing::subscriber::set_default(subscriber(log.clone(), output.clone()));
        let tunnel = log.begin(None);
        log.admit(identity("18"), Some(tunnel), 1).unwrap();
        {
            let span = tracing::info_span!(target:"iroh_relay::server::client","client-connection-actor",connection_id=%18);
            let _entered = span.enter();
            tracing::warn!(target:"iroh_relay::server::client","actor errored Writing a frame failed: timed out after 10s, exiting");
        }
        log.disconnected("18");
        log.finish(tunnel, Ok((30, 40))).await;
        let text = output.text();
        assert!(
            text.contains("WARN")
                && text.contains("relay_transport_error")
                && text.contains("timed out after 10s")
        );
        assert!(text.contains("Reception laptop") && !text.contains("actor errored"));
    }
    #[tokio::test]
    async fn late_handler_error_joins_proxy_failure_without_duplicate_disconnects() {
        let log = Arc::new(RelayLog::default());
        let output = Capture::default();
        let _guard = tracing::subscriber::set_default(subscriber(log.clone(), output.clone()));
        let tunnel = log.begin(None);
        log.admit(identity("19"), Some(tunnel), 1).unwrap();
        log.finish(
            tunnel,
            Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "peer reset socket",
            )),
        )
        .await;
        assert!(!output.text().contains("relay_client_disconnected"));
        {
            let span = tracing::info_span!(target:"iroh_relay::server::client","client-connection-actor",connection_id=%19);
            let _entered = span.enter();
            tracing::warn!(target:"iroh_relay::server::client","actor errored Writing a frame failed: broken pipe, exiting");
        }
        log.disconnected("19");
        let text = output.text();
        assert_eq!(text.matches("relay_client_disconnected").count(), 1);
        assert!(
            text.contains("WARN")
                && text.contains("broken pipe")
                && !text.contains("actor errored")
        );
        assert!(
            !text.contains("bytes_from_client=0"),
            "Failed copies must not invent zero byte counts"
        );
    }
    #[test]
    fn uncorrelated_and_unrelated_warnings_are_never_suppressed() {
        let output = Capture::default();
        let log = Arc::new(RelayLog::default());
        tracing::subscriber::with_default(subscriber(log, output.clone()), || {
            tracing::warn!(target:"iroh_relay::server::client","actor errored Stream terminated, exiting");
            tracing::warn!(target:"iroh_relay::server::client","Other important warning");
        });
        let text = output.text();
        assert!(
            text.contains("actor errored Stream terminated, exiting")
                && text.contains("Other important warning")
        );
    }
    #[tokio::test]
    async fn private_station_aliases_resolve_and_public_relays_do_not_read_them() {
        let dir = tempfile::tempdir().unwrap();
        let station = dir.path();
        std::fs::create_dir(station.join("devices")).unwrap();
        let endpoint = "ab".repeat(32);
        std::fs::write(
            station.join("devices").join(format!("{endpoint}.json")),
            r#"{"id":482913077,"name":"COMPUTER-NAME"}"#,
        )
        .unwrap();
        std::fs::write(
            station.join("shared.json"),
            r#"{"devices":[{"id":482913077,"name":"Reception alias"}]}"#,
        )
        .unwrap();
        let db = Arc::new(Db::open(&station.join("registry.sqlite3")).unwrap());
        let private = Arc::new(RelayLog::default());
        private.configure(db.clone(), Some(station.to_owned()), true);
        assert_eq!(
            private.identity(endpoint.clone()).await,
            Some((482913077, "Reception alias".into()))
        );
        assert!(private.identity("../station.json".into()).await.is_none());
        let public = Arc::new(RelayLog::default());
        public.configure(db, Some(station.to_owned()), false);
        assert!(public.identity(endpoint).await.is_none());
    }
}
