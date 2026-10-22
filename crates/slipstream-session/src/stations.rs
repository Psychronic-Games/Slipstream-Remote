//! A dedicated public endpoint keeps private relay credentials out of public
//! registrations. Both listeners share the host, password and auth throttle.
use crate::{AccountRef, Client, ClientEvent, DirStatus, Inner, ViewerEvent, ViewerSession};
use anyhow::{Context, Result, bail};
use iroh::{Endpoint, SecretKey, Watcher};
use slipstream_core::{
    config::{ServerSettings, Settings},
    directory::{DeviceId, DeviceInfo, DirectoryClient},
    net,
};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{mpsc, watch};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum StationRef {
    #[default]
    Primary,
    Public,
    Account(AccountRef),
}

pub(crate) fn has_custom(settings: &Settings) -> bool {
    settings
        .server
        .url
        .as_deref()
        .is_some_and(|s| !s.trim().is_empty() && !slipstream_core::config::is_public_station(s))
}

pub(crate) struct PublicStation {
    pub endpoint: Endpoint,
    pub settings: Mutex<ServerSettings>,
    pub status: watch::Sender<DirStatus>,
    pub directory: watch::Sender<Option<DirectoryClient>>,
}
impl PublicStation {
    pub async fn bind(
        secret: SecretKey,
        mut server: ServerSettings,
        accepts: bool,
    ) -> Result<Self> {
        server.key = None;
        server.use_public_relays = false;
        server.station_allows_public_relays = false;
        Ok(Self {
            endpoint: net::bind_client(secret, &server, accepts).await?,
            settings: Mutex::new(server),
            status: watch::channel(DirStatus::Connecting).0,
            directory: watch::channel(None).0,
        })
    }
}

impl Client {
    pub fn has_custom_station(&self) -> bool {
        has_custom(&self.settings())
    }

    pub fn station_settings(&self) -> ServerSettings {
        if !self.has_custom_station()
            && let Some(public) = &self.inner.public_station
        {
            return public.settings.lock().unwrap().clone();
        }
        self.settings().server
    }

    pub fn public_dir_status(&self) -> DirStatus {
        self.inner
            .public_station
            .as_ref()
            .map(|s| s.status.borrow().clone())
            .unwrap_or(DirStatus::NotConfigured)
    }

    pub(crate) fn default_endpoint(&self) -> Endpoint {
        if !self.has_custom_station()
            && let Some(public) = &self.inner.public_station
        {
            return public.endpoint.clone();
        }
        self.inner.endpoint.clone()
    }

    pub(crate) fn endpoint_for_address(&self, address: Option<&str>) -> Endpoint {
        if let (Some(address), Some(public)) = (address, &self.inner.public_station) {
            let norm = slipstream_core::directory::normalize_server_url;
            if public
                .settings
                .lock()
                .unwrap()
                .url
                .as_deref()
                .is_some_and(|url| norm(url).ok() == norm(address).ok())
            {
                return public.endpoint.clone();
            }
        }
        if address.is_some() {
            self.inner.endpoint.clone()
        } else {
            self.default_endpoint()
        }
    }

    pub(crate) async fn public_directory(&self) -> Result<DirectoryClient> {
        let public = self
            .inner
            .public_station
            .as_ref()
            .context("Public station is unavailable in this client")?;
        let mut directory = public.directory.subscribe();
        match tokio::time::timeout(Duration::from_secs(10), directory.wait_for(Option::is_some))
            .await
        {
            Ok(Ok(dc)) => Ok(dc.clone().expect("directory available")),
            _ => match public.status.borrow().clone() {
                DirStatus::Error(error) => bail!("Public station: {error}"),
                _ => bail!("The public station is not reachable yet"),
            },
        }
    }

    pub async fn connect_on(
        &self,
        station: &StationRef,
        id: DeviceId,
        password: &str,
        progress: impl Fn(String) + Send + Sync + 'static,
    ) -> Result<(ViewerSession, mpsc::UnboundedReceiver<ViewerEvent>)> {
        match station {
            StationRef::Primary => self.connect(id, password, progress).await,
            StationRef::Account(account) => {
                self.connect_via(account, id, Some(password.into()), progress)
                    .await
            }
            StationRef::Public => {
                progress(format!("Locating {id} on Slipstream Public…"));
                let dc = self.public_directory().await?;
                let found = dc.lookup(id).await?;
                self.connect_found(
                    id,
                    found,
                    password,
                    progress,
                    Some(dc.base_url().to_string()),
                )
                .await
            }
        }
    }
}

fn status(inner: &Inner, value: DirStatus) {
    let public = inner.public_station.as_ref().expect("public station task");
    if *public.status.borrow() != value {
        public.status.send_replace(value.clone());
        inner.emit(ClientEvent::PublicDirectory(value));
    }
}

pub(crate) async fn directory_task(inner: Arc<Inner>) {
    let public = inner.public_station.as_ref().expect("public station task");
    let mut backoff = Duration::from_secs(1);
    loop {
        status(&inner, DirStatus::Connecting);
        let result = run(&inner).await;
        public.directory.send_replace(None);
        if public.endpoint.is_closed() {
            break;
        }
        if let Err(error) = result {
            status(&inner, DirStatus::Error(crate::short_error(&error)));
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}

async fn run(inner: &Inner) -> Result<()> {
    let public = inner.public_station.as_ref().expect("public station task");
    let server = public.settings.lock().unwrap().clone();
    let url = server.url.as_deref().context("Public station has no URL")?;
    let mut directory = DirectoryClient::new(url, None)?;
    let hello = directory.hello().await?;
    net::add_relays(&public.endpoint, &hello.relays).await;
    public.settings.lock().unwrap().relays = hello.relays;
    public.directory.send_replace(Some(directory.clone()));
    if !inner.opts.accept_sessions {
        status(inner, DirStatus::Ready);
        public.endpoint.closed().await;
        return Ok(());
    }
    let _ = tokio::time::timeout(Duration::from_secs(5), public.endpoint.online()).await;
    let mut addresses = public.endpoint.watch_addr();
    let mut heartbeat = tokio::time::interval(Duration::from_secs(
        slipstream_core::directory::HEARTBEAT_SECS,
    ));
    loop {
        tokio::select! {
            _ = heartbeat.tick() => {},
            result = addresses.updated() => { result.context("Public endpoint closed")?; },
        }
        let info = DeviceInfo {
            name: inner.settings.lock().unwrap().device_name.clone(),
            os: slipstream_core::os_name(),
            app_version: slipstream_core::APP_VERSION.into(),
        };
        let id = directory
            .register(public.endpoint.secret_key(), public.endpoint.addr(), info)
            .await?;
        let changed = {
            let mut settings = inner.settings.lock().unwrap();
            let changed = settings.host.public_device_id != Some(id);
            settings.host.public_device_id = Some(id);
            changed
        };
        if changed {
            inner.save_settings();
        }
        status(inner, DirStatus::Online { id });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    struct Station {
        url: String,
        requests: Arc<Mutex<Vec<Value>>>,
        task: tokio::task::JoinHandle<()>,
    }
    impl Drop for Station {
        fn drop(&mut self) {
            self.task.abort();
        }
    }
    impl Station {
        async fn start(key: Option<&str>, relays: Vec<String>) -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
            let (saved, key) = (requests.clone(), key.map(str::to_string));
            let task = tokio::spawn(async move {
                let mut registration = None::<Value>;
                while let Ok((mut socket, _)) = listener.accept().await {
                    let mut bytes = Vec::new();
                    let header_end = loop {
                        let mut buf = [0; 4096];
                        let n = socket.read(&mut buf).await.unwrap();
                        if n == 0 {
                            break 0;
                        }
                        bytes.extend_from_slice(&buf[..n]);
                        if let Some(end) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
                            break end + 4;
                        }
                        assert!(bytes.len() < 64 * 1024);
                    };
                    if header_end == 0 {
                        continue;
                    }
                    let headers = String::from_utf8_lossy(&bytes[..header_end]);
                    let length: usize = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse().unwrap())
                        })
                        .unwrap();
                    while bytes.len() < header_end + length {
                        let mut buf = [0; 4096];
                        let n = socket.read(&mut buf).await.unwrap();
                        assert!(n > 0);
                        bytes.extend_from_slice(&buf[..n]);
                    }
                    let body: Value =
                        serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
                    saved.lock().unwrap().push(body.clone());
                    let result = if key
                        .as_ref()
                        .is_some_and(|k| body["key"].as_str() != Some(k))
                    {
                        json!({"ok":false,"error":"wrong station key"})
                    } else {
                        match body["op"].as_str().unwrap() {
                            "hello" => {
                                json!({"ok":true,"version":2,"time":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),"relays":relays,"public_relays":false})
                            }
                            "register" => {
                                registration = Some(
                                    serde_json::from_str(body["payload"].as_str().unwrap())
                                        .unwrap(),
                                );
                                // Deliberately collide: numeric IDs are station scoped.
                                json!({"ok":true,"id":123456789})
                            }
                            "lookup" => match &registration {
                                Some(reg) => {
                                    json!({"ok":true,"status":"online","addr":reg["addr"],"info":reg["info"]})
                                }
                                None => json!({"ok":true,"status":"not_found"}),
                            },
                            "unregister" => {
                                registration = None;
                                json!({"ok":true})
                            }
                            _ => json!({"ok":true}),
                        }
                    };
                    let body = result.to_string();
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    socket.write_all(response.as_bytes()).await.unwrap();
                }
            });
            Self {
                url,
                requests,
                task,
            }
        }
        fn registration(&self) -> Value {
            let entries = self.requests.lock().unwrap();
            let body = entries
                .iter()
                .rev()
                .find(|r| r["op"] == "register")
                .unwrap();
            serde_json::from_str(body["payload"].as_str().unwrap()).unwrap()
        }
    }
    async fn online(client: &Client, public: bool) {
        tokio::time::timeout(Duration::from_secs(12), async {
            loop {
                let status = if public {
                    client.public_dir_status()
                } else {
                    client.dir_status()
                };
                if matches!(status, DirStatus::Online { .. }) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("station registration timed out");
    }

    #[tokio::test]
    async fn public_and_private_identities_routes_and_restarts_are_isolated() {
        slipstream_core::init_crypto();
        let private = Station::start(
            Some("private-station-key"),
            vec!["http://127.0.0.1:9/?token=private-relay-key".into()],
        )
        .await;
        let public = Station::start(None, vec![]).await;
        let host_home = tempfile::tempdir().unwrap();
        let viewer_home = tempfile::tempdir().unwrap();
        let store = slipstream_core::config::Store::open(host_home.path()).unwrap();
        let mut settings = store.load_settings();
        settings.device_name = "Dual-station host".into();
        settings.server = ServerSettings {
            url: Some(private.url.clone()),
            key: Some("private-station-key".into()),
            use_public_relays: false,
            ..Default::default()
        };
        settings
            .host
            .set_permanent_password(Some("dual-station-password"));
        store.save_settings(&settings).unwrap();
        let public_settings = ServerSettings {
            url: Some(public.url.clone()),
            relays: vec![],
            ..ServerSettings::public_station()
        };
        let opts = options(public_settings.clone(), true);
        let (host, _) = Client::start(store, opts).await.unwrap();
        online(&host, false).await;
        online(&host, true).await;
        let private_id = host.inner.endpoint.id();
        let public_id = host.inner.public_station.as_ref().unwrap().endpoint.id();
        assert_ne!(private_id, public_id);
        assert_eq!(private.registration()["endpoint"], private_id.to_string());
        assert_eq!(public.registration()["endpoint"], public_id.to_string());
        let public_data = serde_json::to_string(&*public.requests.lock().unwrap()).unwrap();
        assert!(
            !public_data.contains("private-station-key")
                && !public_data.contains("private-relay-key")
        );
        assert_eq!(
            host.settings().host.last_device_id,
            host.settings().host.public_device_id
        );
        assert_eq!(host.endpoint_id(), private_id);
        assert_eq!(host.endpoint_for_address(Some(&public.url)).id(), public_id);
        let viewer_store = slipstream_core::config::Store::open(viewer_home.path()).unwrap();
        viewer_store.save_settings(&settings).unwrap();
        let (viewer, _) = Client::start(viewer_store, options(public_settings, false))
            .await
            .unwrap();
        for route in [StationRef::Primary, StationRef::Public] {
            let (session, _) = viewer
                .connect_on(&route, DeviceId(123456789), "dual-station-password", |_| {})
                .await
                .unwrap();
            assert_eq!(session.info().host_name, "Dual-station host");
            session.send(crate::ViewerCmd::Disconnect);
        }
        host.update_settings(|s| {
            s.server = ServerSettings {
                use_public_relays: false,
                ..Default::default()
            }
        });
        assert_eq!(host.endpoint_id(), public_id);
        assert_eq!(
            host.endpoint_for_address(Some(&private.url)).id(),
            private_id
        );
        assert_eq!(host.station_settings().url, Some(public.url.clone()));
        viewer.shutdown().await;
        host.shutdown().await;
        let store = slipstream_core::config::Store::open(host_home.path()).unwrap();
        let opts = options(
            ServerSettings {
                url: Some(public.url.clone()),
                relays: vec![],
                ..ServerSettings::public_station()
            },
            true,
        );
        let (restarted, _) = Client::start(store, opts).await.unwrap();
        online(&restarted, true).await;
        assert_eq!(restarted.endpoint_id(), public_id);
        assert_eq!(
            restarted.settings().host.public_device_id,
            Some(DeviceId(123456789))
        );
        restarted.shutdown().await;
    }
    fn options(public_station: ServerSettings, host: bool) -> crate::ClientOptions {
        crate::ClientOptions {
            public_station: Some(public_station),
            accept_sessions: host,
            test_pattern: true,
            clipboard: slipstream_platform::clipboard::ClipboardSource::Memory(Default::default()),
            audio_out: crate::AudioOut::Memory(Default::default()),
        }
    }
}
