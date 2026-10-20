//! Remote LAN survey, in table and logical topology views.

use super::{theme, theme::pal};
use eframe::egui::{self, Align2, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use slipstream_core::network::{self, DeviceName, NetworkDevice, NetworkInterface, ScanEvent, ScanRequest};
use slipstream_session::{RemoteFs, network::RemoteScan};
use std::{collections::{BTreeMap, BTreeSet}, net::Ipv4Addr, sync::{Arc, Mutex}, time::Instant};

type Adapters = Result<Vec<NetworkInterface>, String>;

pub struct NetworkScanner {
    pub open: bool,
    fs: RemoteFs,
    adapters: Vec<NetworkInterface>,
    adapter_rx: Option<tokio::sync::oneshot::Receiver<Adapters>>,
    adapter_job: Option<tokio::task::JoinHandle<()>>,
    loaded: bool,
    selected: usize,
    cidr: String,
    ports: String,
    topology: bool,
    scan: Option<RemoteScan>,
    devices: BTreeMap<Ipv4Addr, NetworkDevice>,
    /// Adapter and range used for these results, kept separate from the form.
    surveyed: Option<(NetworkInterface, String)>,
    surveyed_ports: Vec<u16>,
    completed: u32,
    total: u32,
    status: String,
    error: Option<String>,
    started: Option<Instant>,
    elapsed: f32,
    filter: String,
    selected_device: Option<Ipv4Addr>,
    names: BTreeMap<Ipv4Addr, DeviceName>,
    names_requested: BTreeSet<Ipv4Addr>,
    names_rx: Option<tokio::sync::oneshot::Receiver<Result<Vec<DeviceName>, String>>>,
    names_job: Option<tokio::task::JoinHandle<()>>,
    names_notice: Option<String>,
    can_open_web: bool,
    web_rx: Option<tokio::sync::oneshot::Receiver<Result<String, String>>>,
    web_job: Option<tokio::task::JoinHandle<()>>,
    web_notice: Option<Result<String, String>>,
    can_tunnel: bool,
    support_rx: Option<tokio::sync::oneshot::Receiver<Result<ToolReply, String>>>,
    support_job: Option<tokio::task::JoinHandle<()>>,
    support_notice: Option<Result<String, String>>,
    diagnostic: Vec<slipstream_core::proto::InfoSection>,
    diagnostic_name: String,
    comparison_dns: String,
    wake_mac: String,
    tunnel_port: u16,
    tunnels: Vec<slipstream_session::tunnel::Tunnel>,
}

enum ToolReply {
    Message(String),
    Diagnostic(Vec<slipstream_core::proto::InfoSection>),
    Tunnel(slipstream_session::tunnel::Tunnel, u16),
}

impl Drop for NetworkScanner {
    fn drop(&mut self) {
        if let Some(job) = self.support_job.take() { job.abort(); }
        if let Some(job) = self.web_job.take() { job.abort(); }
        if let Some(job) = self.names_job.take() { job.abort(); }
        if let Some(job) = self.adapter_job.take() {
            job.abort();
        }
    }
}

impl NetworkScanner {
    pub fn close(&mut self) {
        self.open=false;
        self.disconnect();
    }
    fn cancel_requests(&mut self, reason: &str) {
        self.stop();
        self.cancel_support(reason);
        if let Some(job)=self.adapter_job.take() {job.abort();}
        self.adapter_rx=None;
        if let Some(job)=self.web_job.take() {job.abort();}
        if self.web_rx.take().is_some() {self.web_notice=Some(Err(reason.into()));}
    }
    fn cancel_support(&mut self, reason: &str) {
        if let Some(job)=self.support_job.take() {job.abort();}
        if self.support_rx.take().is_some() {self.support_notice=Some(Err(reason.into()));}
    }
    fn hide(&mut self) {
        self.open=false;
        // Existing tunnels belong to the session. One-shot requests belong to
        // this window and must not open a browser later when it is reopened.
        self.cancel_requests("Window closed; pending requests cancelled. Already-sent actions cannot be recalled");
    }
    pub fn disconnect(&mut self) {
        self.cancel_requests("Link interrupted; the request was not confirmed");
        self.tunnels.clear();
    }
    pub fn reconnect(&mut self,fs:RemoteFs,can_open_web:bool,can_tunnel:bool) {
        self.disconnect();
        self.adapters.clear();self.loaded=false;
        self.fs=fs;self.can_open_web=can_open_web;self.can_tunnel=can_tunnel;
    }

    pub fn new(fs: RemoteFs, can_open_web: bool, can_tunnel: bool) -> Self {
        Self {
            open: std::env::var_os("SLIPSTREAM_TEST_NETWORK").is_some(),
            fs,
            adapters: Vec::new(),
            adapter_rx: None,
            adapter_job: None,
            loaded: false,
            selected: 0,
            cidr: String::new(),
            ports: network::COMMON_PORTS.into(),
            topology: false,
            scan: None,
            devices: BTreeMap::new(),
            surveyed: None,
            surveyed_ports: Vec::new(),
            completed: 0,
            total: 0,
            status: "Choose the remote adapter and press Scan".into(),
            error: None,
            started: None,
            elapsed: 0.0,
            filter: String::new(),
            selected_device: None,
            names: BTreeMap::new(), names_requested: BTreeSet::new(), names_rx: None,
            names_job: None, names_notice: None,
            can_open_web, web_rx: None, web_job: None, web_notice: None,
            can_tunnel, support_rx:None,support_job:None,support_notice:None,
            diagnostic:Vec::new(),diagnostic_name:"example.com".into(),comparison_dns:String::new(),
            wake_mac:String::new(),tunnel_port:80,tunnels:Vec::new(),
        }
    }

    fn load(&mut self, ctx: &egui::Context, rt: &tokio::runtime::Runtime) {
        self.error = None;
        let (tx, rx) = tokio::sync::oneshot::channel();
        let fs = self.fs.clone();
        let wake = ctx.clone();
        self.adapter_rx = Some(rx);
        self.adapter_job = Some(rt.spawn(async move {
            let result =
                tokio::time::timeout(std::time::Duration::from_secs(10), fs.network_interfaces())
                    .await
                    .map_err(|_| {
                        "The remote computer did not answer; it may need a newer Slipstream"
                            .to_string()
                    })
                    .and_then(|r| r.map_err(|e| format!("{e:#}")));
            let _ = tx.send(result);
            wake.request_repaint();
        }));
    }

    fn request(&self) -> Result<ScanRequest, String> {
        let adapter = self
            .adapters
            .get(self.selected)
            .ok_or("No IPv4 adapter selected")?;
        let request = ScanRequest {
            interface: adapter.address,
            cidr: self.cidr.clone(),
            ports: network::parse_ports(&self.ports).map_err(|e| e.to_string())?,
        };
        network::validate(&request, &self.adapters).map_err(|e| e.to_string())?;
        Ok(request)
    }

    fn poll(&mut self) {
        if let Some(rx) = &mut self.adapter_rx {
            match rx.try_recv() {
                Ok(result) => {
                    self.adapter_rx = None;
                    self.adapter_job = None;
                    self.loaded = true;
                    match result {
                        Ok(adapters) => {
                            self.adapters = adapters;
                            self.selected = 0;
                            self.cidr = self
                                .adapters
                                .first()
                                .map(NetworkInterface::cidr)
                                .unwrap_or_default();
                            if self.adapters.is_empty() {
                                self.error = Some(
                                    "The remote computer has no connected IPv4 adapters".into(),
                                );
                            }
                        }
                        Err(e) => self.error = Some(e),
                    }
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    self.adapter_rx = None;
                    self.loaded = true;
                    self.error = Some("The adapter request ended without an answer".into());
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
            }
        }
        let mut finished = false;
        if let Some(scan) = &mut self.scan {
            while let Ok(event) = scan.events.try_recv() {
                match event {
                    Ok(ScanEvent::Started { total }) => {
                        self.total = total;
                        self.status = "Scanning from the remote computer".into();
                    }
                    Ok(ScanEvent::Device(device)) => {
                        self.devices.insert(device.address, device);
                    }
                    Ok(ScanEvent::Progress { completed, total }) => {
                        self.completed = completed;
                        self.total = total;
                    }
                    Ok(ScanEvent::Finished) => {
                        self.status = "Scan complete".into();
                        finished = true;
                    }
                    Err(e) => {
                        self.error = Some(e);
                        self.status = "Scan failed · partial results retained".into();
                        finished = true;
                    }
                }
            }
            if scan.events.is_closed() && !finished {
                self.error = Some("The remote scan ended without a completion message".into());
                self.status = "Scan interrupted · partial results retained".into();
                finished = true;
            }
        }
        if let Some(started) = self.started {
            self.elapsed = started.elapsed().as_secs_f32();
        }
        if finished {
            self.scan = None;
            self.started = None;
        }
    }

    fn stop(&mut self) {
        let busy = self.scan.is_some() || self.names_rx.is_some();
        self.scan = None;
        self.started = None;
        self.cancel_names();
        self.names_requested.extend(self.devices.keys().copied());
        if busy { self.status = "Stopped · partial results retained".into(); }
    }

    fn cancel_names(&mut self) {
        if let Some(job) = self.names_job.take() { job.abort(); }
        self.names_rx = None;
    }

    fn poll_names(&mut self, ctx: &egui::Context, rt: &tokio::runtime::Runtime, connected: bool) {
        if let Some(rx) = &mut self.names_rx {
            match rx.try_recv() {
                Ok(result) => {
                    self.names_rx = None; self.names_job = None;
                    match result {
                        Ok(names) => { for name in names { self.names.insert(name.address,name); } },
                        Err(_) => self.names_notice = Some("Device-name discovery unavailable; the remote computer may need an update. Port results are still available.".into()),
                    }
                },
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    self.names_rx = None; self.names_job = None;
                    self.names_notice = Some("Device-name discovery was interrupted".into());
                },
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {},
            }
        }
        if !connected || self.names_rx.is_some() || self.names_notice.is_some() { return; }
        let Some((interface,_)) = &self.surveyed else { return };
        let addresses: Vec<_> = self.devices.keys().filter(|ip| !self.names_requested.contains(ip)).copied().take(16).collect();
        if addresses.is_empty() { return; }
        self.names_requested.extend(addresses.iter().copied());
        let (fs,source,wake) = (self.fs.clone(),interface.address,ctx.clone());
        let (tx,rx) = tokio::sync::oneshot::channel();
        self.names_rx = Some(rx);
        self.names_job = Some(rt.spawn(async move {
            let result = tokio::time::timeout(std::time::Duration::from_secs(20), fs.network_names(source,addresses)).await
                .map_err(|_| "Name lookup timed out".to_string()).and_then(|r|r.map_err(|e|format!("{e:#}")));
            let _ = tx.send(result);wake.request_repaint();
        }));
    }

    /// Each session owns a desktop viewport, independent of the main window.
    /// On Android, egui embeds it because that backend has no extra windows.
    pub fn show(shared: &Arc<Mutex<Self>>, ctx: &egui::Context, rt: &Arc<tokio::runtime::Runtime>, id: u64, host: &str) {
        if !shared.lock().expect("network window").open { return; }
        let (shared, rt, host) = (shared.clone(), rt.clone(), host.to_string());
        let viewport = egui::ViewportId::from_hash_of(("network-scan", id));
        let builder = egui::ViewportBuilder::default()
            .with_decorations(false)
            .with_transparent(theme::transparent_window())
            .with_title(format!("Network scan · {host} · Slipstream Remote"))
            .with_app_id("slipstream-remote")
            .with_icon({
                static ICON: std::sync::OnceLock<Arc<egui::IconData>> = std::sync::OnceLock::new();
                ICON.get_or_init(|| Arc::new(theme::icon(64))).clone()
            })
            .with_inner_size([1100.0,760.0]).with_min_inner_size([520.0,360.0]);
        ctx.show_viewport_deferred(viewport, builder, move |ui, class| {
            let mut scanner = shared.lock().expect("network window");
            let ctx = ui.ctx().clone();
            let connected=scanner.fs.is_connected();
            if !scanner.open {
                theme::dismiss_tool_viewport(&ctx, class==egui::ViewportClass::Deferred);
                return;
            }
            scanner.tick(&ctx, &rt, connected);
            let (body, close)=theme::tool_frame(ui,&format!("Network scan · {host}"),class == egui::ViewportClass::Deferred,pal::CYAN);
            let ui=&mut ui.new_child(egui::UiBuilder::new().max_rect(body));
            if close || ui.input(|i|i.viewport().close_requested()) {
                scanner.hide();
                theme::dismiss_tool_viewport(&ctx, class==egui::ViewportClass::Deferred);
                return;
            }
            egui::ScrollArea::vertical().id_salt("scan-body").auto_shrink([false,false])
                .show(ui, |ui| scanner.contents(ui, &ctx, &rt, &host, connected));
        });
    }

    fn tick(&mut self, ctx: &egui::Context, rt: &tokio::runtime::Runtime, connected: bool) {
        if !connected {
            self.disconnect(); self.status = "Session ended · results retained".into();
        }
        if !self.loaded && self.adapter_rx.is_none() && connected { self.load(ctx,rt); }
        self.poll();
        if let Some(rx) = &mut self.web_rx {
            match rx.try_recv() {
                Ok(result) => { self.web_notice = Some(result); self.web_rx = None; self.web_job = None; },
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    self.web_notice = Some(Err("The remote browser request ended without an answer".into()));
                    self.web_rx = None; self.web_job = None;
                },
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {},
            }
        }
        if let Some(rx) = &mut self.support_rx {
            match rx.try_recv() {
                Ok(result) => {
                    self.support_rx = None;self.support_job = None;
                    self.support_notice = Some(match result {
                        Ok(ToolReply::Message(text)) => Ok(text),
                        Ok(ToolReply::Diagnostic(sections)) => {self.diagnostic = sections;Ok("Diagnostic checks finished; individual results are below".into())},
                        Ok(ToolReply::Tunnel(tunnel, port)) => {
                            let endpoint = format!("127.0.0.1:{}", tunnel.local_port);
                            if matches!(port, 80 | 443) {
                                let scheme = if port == 443 {"https"} else {"http"};
                                ctx.open_url(egui::OpenUrl::new_tab(format!("{scheme}://{endpoint}/")));
                            }
                            let text = format!("Connected through this workstation: {endpoint} → {}:{port}", tunnel.host);
                            self.tunnels.push(tunnel);Ok(text)
                        },
                        Err(e) => Err(e),
                    });
                },
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    self.support_rx=None;self.support_job=None;self.support_notice=Some(Err("The support request ended without an answer".into()));
                },
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {},
            }
        }
        self.poll_names(ctx,rt,connected);
        if self.scan.is_some() || self.adapter_rx.is_some() || self.names_rx.is_some() || self.web_rx.is_some() || self.support_rx.is_some() { ctx.request_repaint_after(std::time::Duration::from_millis(100)); }
    }

    fn contents(&mut self, ui: &mut Ui, ctx: &egui::Context, rt: &tokio::runtime::Runtime, host: &str, connected: bool) {
                self.controls(ui, ctx, rt, connected);
                ui.add_space(5.0);
                ui.horizontal_wrapped(|ui| {
                    for (label, topology) in [("TABLE", false), ("TOPOLOGY", true)] {
                        let selected = self.topology == topology;
                        let button = egui::Button::new(RichText::new(label).font(theme::display_bold(17.0))
                            .color(if selected { pal::BG } else { pal::LAVENDER }))
                            .fill(if selected { pal::CYAN } else { pal::PANEL })
                            .stroke(Stroke::new(1.0, if selected { pal::CYAN } else { pal::LAVENDER }))
                            .corner_radius(16.0).min_size(vec2(86.0, 32.0));
                        if ui.add(button).clicked() { self.topology = topology; }
                    }
                    ui.label(RichText::new(format!("{} DEVICE{}", self.devices.len(), if self.devices.len() == 1 { "" } else { "S" })).font(theme::display(18.0)).color(pal::GOLD));
                    ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("Filter name, IP, MAC, port or service").desired_width(210.0));
                    if ui.add_enabled(!self.devices.is_empty(), egui::Button::new("Copy results")).on_hover_text("Copy the filtered devices, scan scope and tested ports as text").clicked() { ui.ctx().copy_text(self.export(host)); }
                });
                if self.total > 0 {
                    ui.add(egui::ProgressBar::new(self.completed as f32 / self.total as f32).fill(pal::VIOLET)
                        .text(format!("{} · {}/{} address{} · {:.1}s", self.status, self.completed, self.total, if self.total == 1 { "" } else { "es" }, self.elapsed)));
                } else { ui.label(RichText::new(&self.status).color(pal::DIM)); }
                if self.names_rx.is_some() { ui.label(RichText::new("Looking up device names…").small().color(pal::CYAN)); }
                if let Some(note) = &self.names_notice { ui.label(RichText::new(note).small().color(pal::GOLD)); }
                if let Some(error) = &self.error { ui.label(RichText::new(error).color(pal::RED)); }
                if let Some((interface, cidr)) = &self.surveyed {
                    ui.label(RichText::new(format!("Survey: {cidr} · {} · source {} · gateway {}", interface.name, interface.address, interface.gateway.map(|g| g.to_string()).unwrap_or_else(|| "not reported".into()))).small().color(pal::BLUE));
                }
                if self.web_rx.is_some() { ui.label(RichText::new("Opening the remote browser…").color(pal::CYAN)); }
                if let Some(notice) = &self.web_notice {
                    let (text,color) = match notice { Ok(text) => (text,pal::GREEN), Err(text) => (text,pal::RED) };
                    ui.label(RichText::new(text).color(color));
                }
                let web_enabled = connected && self.can_open_web && self.web_rx.is_none();
                let mut open_web = None;
                if let Some(address) = self.selected_device
                    && let Some(device) = self.devices.get(&address) {
                    ui.label(RichText::new(format!("{} · {} · {} · {}", self.names.get(&address).map(|n|n.name.as_str()).unwrap_or("Name not reported"), device.address, device.mac.as_deref().unwrap_or("MAC unavailable"), self.role(address))).color(pal::TAN));
                    open_web = ports_ui(ui, device, web_enabled);
                }
                self.support_tools(ui, ctx, rt, connected);
                ui.add_space(5.0);
                egui::ScrollArea::both().id_salt("scan-results").auto_shrink([false, false])
                    .max_height((ui.available_height() - 80.0).max(100.0))
                    .show(ui, |ui| { if self.topology { self.topology(ui, host); } else if let Some(target) = self.table(ui, web_enabled) { open_web = Some(target); } });
                if let Some((address,port)) = open_web {
                    self.open_web(address, port, ctx, rt);
                }
                ui.label(RichText::new("80 HTTP / 443 HTTPS open in the REMOTE computer's browser; select a topology node for its port links.").small().color(pal::BLUE));
                ui.label(RichText::new("Device names are best effort · TCP service names follow port conventions · silent devices may be missed").small().color(pal::DIM));
                ui.label(RichText::new("Logical scan map: the remote gateway and discovered addresses; physical links are not discovered.").small().color(pal::DIM));
    }

    fn controls(
        &mut self,
        ui: &mut Ui,
        ctx: &egui::Context,
        rt: &tokio::runtime::Runtime,
        connected: bool,
    ) {
        let busy = self.scan.is_some();
        ui.add_enabled_ui(!busy && self.support_rx.is_none() && connected, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("REMOTE ADAPTER").color(pal::CYAN));
                let mut selected = self.selected;
                egui::ComboBox::from_id_salt("scan-adapter")
                    .width(ui.available_width().min(280.0))
                    .selected_text(
                        self.adapters
                            .get(selected)
                            .map(|a| format!("{} · {}", a.name, a.address))
                            .unwrap_or_else(|| "Loading adapters…".into()),
                    )
                    .show_ui(ui, |ui| {
                        for (index, adapter) in self.adapters.iter().enumerate() {
                            ui.selectable_value(
                                &mut selected,
                                index,
                                format!(
                                    "{} · {}/{}",
                                    adapter.name, adapter.address, adapter.prefix
                                ),
                            );
                        }
                    });
                if selected != self.selected {
                    self.selected = selected;
                    self.cidr = self.adapters[selected].cidr();
                }
                if ui
                    .add_enabled(
                        self.adapter_rx.is_none(),
                        egui::Button::new("Refresh adapters"),
                    )
                    .clicked()
                {
                    self.load(ctx, rt);
                }
            });
            if let Some(adapter) = self.adapters.get(self.selected) {
                ui.label(
                    RichText::new(format!(
                        "Subnet mask {} · gateway {}",
                        Ipv4Addr::from(network::mask(adapter.prefix)),
                        adapter
                            .gateway
                            .map(|g| g.to_string())
                            .unwrap_or_else(|| "not reported".into())
                    ))
                    .small()
                    .color(pal::DIM),
                );
            }
            ui.horizontal_wrapped(|ui| {
                ui.label("Target subnet");
                ui.add(
                    egui::TextEdit::singleline(&mut self.cidr)
                        .desired_width(170.0)
                        .hint_text("192.168.1.0/24"),
                ).on_hover_text("Enter any reachable subnet or VLAN; scans run from the selected remote adapter");
                ui.label("TCP ports");
                ui.add(
                    egui::TextEdit::singleline(&mut self.ports)
                        .desired_width(ui.available_width().clamp(160.0, 300.0))
                        .hint_text("22,80,443,3389 or 1-1024"),
                );
                if ui.button("Common ports").clicked() {
                    self.ports = network::COMMON_PORTS.into();
                }
            });
        });
        let request = self.request();
        ui.horizontal_wrapped(|ui| {
            if busy {
                if theme::pill(ui, "Stop scan", pal::RED).clicked() {
                    self.stop();
                }
            } else if ui
                .add_enabled(
                    connected && request.is_ok(),
                    egui::Button::new(RichText::new("SCAN NETWORK").color(pal::CYAN)),
                )
                .clicked()
                && let Ok(request) = request.clone()
            {
                self.cancel_names();
                self.names.clear(); self.names_requested.clear(); self.names_notice = None;
                self.devices.clear();
                self.selected_device = None;
                self.error = None;
                self.completed = 0;
                self.total = 0;
                self.elapsed = 0.0;
                self.surveyed = self
                    .adapters
                    .get(self.selected)
                    .cloned()
                    .map(|a| (a, request.cidr.clone()));
                self.surveyed_ports = request.ports.clone();
                self.started = Some(Instant::now());
                self.status = "Starting remote scan…".into();
                let _runtime = rt.enter();
                self.scan = Some(self.fs.network_scan(request));
            }
            if !busy && !self.adapters.is_empty() {
                match &request {
                    Ok(r) => {
                        let (a, b) = network::subnet(&r.cidr).expect("validated subnet");
                        ui.label(
                            RichText::new(format!(
                                "{} address{} · {} TCP ports",
                                b - a + 1,
                                if a == b { "" } else { "es" },
                                r.ports.len()
                            ))
                            .small()
                            .color(pal::DIM),
                        );
                    }
                    Err(e) => {
                        ui.label(RichText::new(e).small().color(pal::GOLD));
                    }
                }
            }
        });
    }

    fn visible(&self) -> Vec<NetworkDevice> {
        let filter = self.filter.trim().to_lowercase();
        self.devices
            .values()
            .filter(|d| {
                filter.is_empty()
                    || format!(
                        "{} {} {} {}",
                        d.address,
                        self.names.get(&d.address).map(|n| n.name.as_str()).unwrap_or(""),
                        d.mac.as_deref().unwrap_or(""),
                        ports_text(&d.open_ports)
                    )
                    .to_lowercase()
                    .contains(&filter)
            })
            .cloned()
            .collect()
    }

    fn role(&self, address: Ipv4Addr) -> &'static str {
        let Some((interface, _)) = &self.surveyed else {
            return "Device";
        };
        if address == interface.address {
            "Remote host"
        } else if Some(address) == interface.gateway {
            "Gateway"
        } else {
            "Device"
        }
    }

    fn select_device(&mut self,address:Ipv4Addr) {
        if self.selected_device!=Some(address) {
            if let Some(device)=self.devices.get(&address) {
                self.tunnel_port=[80,443,3389,22].into_iter().find(|p|device.open_ports.contains(p))
                    .or_else(||device.open_ports.first().copied()).unwrap_or(80);
            }
            self.selected_device=Some(address);
        }
    }

    fn table(&mut self, ui: &mut Ui, web_enabled: bool) -> Option<(Ipv4Addr,u16)> {
        let mut open_web = None;
        let devices = self.visible();
        if devices.is_empty() {
            ui.label(RichText::new("No matching devices yet").color(pal::DIM));
            return None;
        }
        egui::Grid::new("network-table").num_columns(6).striped(true).spacing([20.0, 10.0]).show(ui, |ui| {
            for title in ["IP ADDRESS", "DEVICE NAME", "ROLE", "MAC ADDRESS", "DISCOVERY", "OPEN TCP PORTS"] { ui.label(RichText::new(title).font(theme::display(17.0)).color(pal::CYAN)); }
            ui.end_row();
            for device in devices {
                if ui.selectable_label(self.selected_device == Some(device.address), RichText::new(device.address.to_string()).monospace().color(pal::TAN)).clicked() { self.select_device(device.address); }
                let name = self.names.get(&device.address);
                ui.label(RichText::new(name.map(|n| n.name.as_str()).unwrap_or("—")).color(pal::TAN))
                    .on_hover_text(name.map(|n| n.source.as_str()).unwrap_or("No name was reported by DNS, NetBIOS or mDNS"));
                ui.label(RichText::new(self.role(device.address)).color(pal::LAVENDER));
                ui.label(RichText::new(device.mac.as_deref().unwrap_or("—")).monospace().color(pal::DIM));
                ui.label(RichText::new(if device.responded { "Responded" } else { "Neighbor entry" }).color(if device.responded { pal::GREEN } else { pal::GOLD }))
                    .on_hover_text(if device.responded { "Answered TCP or ICMP" } else { "In the remote neighbor cache; no TCP or ICMP answer confirmed it during this scan" });
                ui.vertical(|ui| { ui.set_max_width(330.0); if let Some(target) = ports_ui(ui, &device, web_enabled) { open_web = Some(target); } });
                ui.end_row();
            }
        });
        open_web
    }

    fn open_web(&mut self, address: Ipv4Addr, port: u16, ctx: &egui::Context, rt: &tokio::runtime::Runtime) {
        if self.web_rx.is_some() { return; }
        let (tx,rx) = tokio::sync::oneshot::channel();
        let (fs,ctx) = (self.fs.clone(),ctx.clone());
        self.web_rx = Some(rx);
        self.web_notice = None;
        self.web_job = Some(rt.spawn(async move {
            let result = fs.open_network_web(address,port).await
                .map(|()| format!("Sent {} to the remote browser", network::web_url(address,port).unwrap_or_default()))
                .map_err(|e|format!("{e:#}"));
            let _ = tx.send(result);ctx.request_repaint();
        }));
    }

    fn start_tool(&mut self, ctx: &egui::Context, rt: &tokio::runtime::Runtime, job: impl std::future::Future<Output=anyhow::Result<ToolReply>> + Send + 'static) {
        if self.support_rx.is_some() {return;}
        let (tx, rx) = tokio::sync::oneshot::channel();let ctx=ctx.clone();
        self.support_rx=Some(rx);self.support_notice=None;
        self.support_job=Some(rt.spawn(async move {let result=job.await.map_err(|e|format!("{e:#}"));let _=tx.send(result);ctx.request_repaint();}));
    }

    fn support_tools(&mut self, ui: &mut Ui, ctx: &egui::Context, rt: &tokio::runtime::Runtime, connected: bool) {
        let busy=self.support_rx.is_some();
        let mut connect=None;let mut wake=false;let mut diagnose=false;
        ui.collapsing("Connect through / Wake-on-LAN / Diagnostics", |ui| {
            if let Some(device)=self.selected_device.and_then(|ip|self.devices.get(&ip)) {
                ui.horizontal_wrapped(|ui| {
                    ui.label(format!("Connect to {}",device.address));
                    egui::ComboBox::from_id_salt("tunnel-service").selected_text(self.tunnel_port.to_string()).show_ui(ui,|ui| {
                        for port in &device.open_ports {ui.selectable_value(&mut self.tunnel_port,*port,format!("{port} · {}",network::service(*port)));}
                    });
                    ui.add(egui::DragValue::new(&mut self.tunnel_port).range(1..=65535).prefix("TCP "));
                    if ui.add_enabled(connected && self.can_tunnel && !busy,egui::Button::new("Connect from my computer")).on_hover_text("Requires Port forwarding on the remote host. HTTP/HTTPS open locally; other services use the copied loopback endpoint.").clicked() {connect=Some((device.address,self.tunnel_port));}
                    if let Some(mac)=&device.mac && ui.button("Use MAC for wake").clicked() {self.wake_mac=mac.clone();}
                });
            } else {ui.label("Select a discovered device to connect through this workstation.");}
            if !self.can_tunnel {ui.label(RichText::new("Enable Port forwarding in the remote host's Config to connect through it.").small().color(pal::GOLD));}
            ui.label(RichText::new("Tunnels listen only on this computer's loopback address and end with this link. Some web interfaces need their original hostname/certificate or redirect configuration.").small().color(pal::DIM));
            let mut remove=None;
            for (i,tunnel) in self.tunnels.iter().enumerate() {
                ui.horizontal_wrapped(|ui| {
                    let endpoint=format!("127.0.0.1:{}",tunnel.local_port);
                    ui.label(format!("{endpoint} → {}:{}",tunnel.host,tunnel.port));
                    if ui.button("Copy endpoint").clicked() {ctx.copy_text(endpoint);}
                    if ui.button("Close tunnel").clicked() {remove=Some(i);}
                });
                if let Some(error)=tunnel.status().last_error {ui.label(RichText::new(error).color(pal::RED));}
            }
            if let Some(i)=remove {self.tunnels.remove(i);}
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                ui.label("Wake MAC");ui.add(egui::TextEdit::singleline(&mut self.wake_mac).desired_width(180.0).hint_text("00:11:22:33:44:55"));
                wake=ui.add_enabled(connected && self.can_open_web && !busy,egui::Button::new("Send wake packet")).clicked();
            });
            ui.label(RichText::new("Uses the selected adapter's LAN broadcast. The host must allow Wake-on-LAN, Control and Network scan; the sleeping device must have wake enabled. Packet delivery does not confirm it woke.").small().color(pal::DIM));
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                ui.label("DNS name");ui.add(egui::TextEdit::singleline(&mut self.diagnostic_name).desired_width(190.0));
                ui.label("Compare resolver");ui.add(egui::TextEdit::singleline(&mut self.comparison_dns).desired_width(160.0).hint_text("optional IP"));
                diagnose=ui.add_enabled(connected && !busy && self.scan.is_none(),egui::Button::new("Run diagnostics")).clicked();
            });
            ui.label(RichText::new("Queries the host's configured resolvers and optional comparison resolver, checks gateway ICMP and verifies its station API. The DNS name is sent to those resolvers.").small().color(pal::DIM));
            for section in &self.diagnostic {for (key,value) in &section.rows {ui.label(format!("{key}: {value}"));}}
            if !self.diagnostic.is_empty() && ui.button("Copy diagnostics").clicked() {ctx.copy_text(self.diagnostic_text());}
        });
        if busy {ui.horizontal(|ui| {ui.spinner();ui.label("Running support request…");if ui.button("Stop waiting").clicked() {self.cancel_support("Stopped waiting; an already-sent action cannot be recalled");}});}
        if let Some(result)=&self.support_notice {match result {Ok(text)=>{ui.label(RichText::new(text).color(pal::GREEN));},Err(text)=>{ui.label(RichText::new(text).color(pal::RED));}}}
        if let Some((address,port))=connect {let fs=self.fs.clone();self.start_tool(ctx,rt,async move {Ok(ToolReply::Tunnel(fs.connect_through(address,port).await?,port))});}
        if wake || diagnose {
            let Some(source)=self.adapters.get(self.selected).map(|a|a.address) else {self.support_notice=Some(Err("Choose a connected remote adapter first".into()));return;};
            let fs=self.fs.clone();
            if wake {let mac=self.wake_mac.clone();self.start_tool(ctx,rt,async move {fs.wake_on_lan(source,mac).await?;Ok(ToolReply::Message("Sent three LAN wake packets; device startup is not confirmed".into()))});}
            else {
                let name=self.diagnostic_name.trim().to_string();
                let comparison=if self.comparison_dns.trim().is_empty(){None}else{match self.comparison_dns.trim().parse(){Ok(ip)=>Some(ip),Err(_)=>{self.support_notice=Some(Err("Enter a numeric comparison DNS resolver IP".into()));return;}}};
                self.start_tool(ctx,rt,async move {Ok(ToolReply::Diagnostic(fs.network_diagnostics(source,name,comparison).await?))});
            }
        }
    }

    pub fn diagnostic_text(&self)->String {
        self.diagnostic.iter().map(|s|format!("{}\n{}",s.title,s.rows.iter().map(|(k,v)|format!("{k}: {v}\n")).collect::<String>())).collect()
    }

    fn topology(&mut self, ui: &mut Ui, host: &str) {
        let Some((interface,cidr)) = self.surveyed.clone() else {
            ui.label(RichText::new("Run a scan to chart the remote network").color(pal::DIM));return;
        };
        let width=ui.available_width().max(280.0);
        let columns=((width/245.0) as usize).clamp(1,4);
        let devices=topology_members(self.visible(),interface.gateway);
        let rows=devices.len().div_ceil(columns);
        let (area,_) = ui.allocate_exact_size(vec2(width,180.0+rows as f32*120.0),Sense::hover());
        let painter=ui.painter().with_clip_rect(ui.clip_rect());
        let center=area.center().x;
        let gateway_rect=Rect::from_center_size(pos2(center,area.top()+45.0),vec2(310.0_f32.min(width-12.0),82.0));
        painter.rect_filled(gateway_rect,18.0,pal::GOLD);
        let gateway_text=interface.gateway.map(|ip|format!("REMOTE GATEWAY   {ip}")).unwrap_or_else(||"GATEWAY NOT REPORTED".into());
        let gp=painter.with_clip_rect(gateway_rect.shrink(8.0).intersect(ui.clip_rect()));
        gp.text(pos2(center,gateway_rect.top()+12.0),Align2::CENTER_TOP,gateway_text,theme::display_bold(18.0),pal::BG);
        if let Some(address)=interface.gateway {
            let device=self.devices.get(&address);
            let name=self.names.get(&address).map(|n|n.name.as_str()).unwrap_or("Name not reported");
            gp.text(pos2(center,gateway_rect.top()+35.0),Align2::CENTER_TOP,name,theme::body(17.0),pal::BG);
            gp.text(pos2(center,gateway_rect.top()+59.0),Align2::CENTER_TOP,
                device.map(|d|format!("{} open TCP ports",d.open_ports.len())).unwrap_or_else(||"Reported by the remote adapter".into()),theme::mono(12.0),pal::BG);
            let detail=device.map(|d|ports_text(&d.open_ports)).unwrap_or_else(||"Gateway not included in scan results".into());
            if ui.interact(gateway_rect,ui.scope_id().with("gateway"),Sense::click()).on_hover_text(format!("{address}\n{name}\n{detail}")).clicked() { self.select_device(address); }
        }
        let bus_y=area.top()+130.0;
        painter.line_segment([pos2(center,gateway_rect.bottom()),pos2(center,bus_y)],Stroke::new(2.0,pal::GOLD));
        painter.line_segment([pos2(area.left()+10.0,bus_y),pos2(area.right()-10.0,bus_y)],Stroke::new(5.0,pal::VIOLET));
        painter.text(pos2(area.left()+12.0,bus_y-10.0),Align2::LEFT_BOTTOM,format!("SCAN RANGE   {cidr}"),theme::mono(15.0),pal::LAVENDER);
        for (index,device) in devices.iter().enumerate() {
            let (col,row)=(index%columns,index/columns);
            let x=area.left()+(col as f32+0.5)*width/columns as f32;
            let top=area.top()+175.0+row as f32*120.0;
            let card=Rect::from_center_size(pos2(x,top+41.0),vec2((width/columns as f32-18.0).min(300.0),92.0));
            if !card.expand(20.0).intersects(ui.clip_rect()) { continue; }
            let row_bus=top-15.0;
            if row>0 {
                painter.line_segment([pos2(area.left()+5.0,bus_y),pos2(area.left()+5.0,row_bus)],Stroke::new(1.0,pal::VIOLET));
                painter.line_segment([pos2(area.left()+5.0,row_bus),pos2(area.right()-10.0,row_bus)],Stroke::new(1.0,pal::VIOLET));
            }
            painter.line_segment([pos2(x,if row==0 {bus_y}else{row_bus}),pos2(x,card.top())],Stroke::new(1.0,pal::VIOLET));
            let color=if device.address==interface.address {pal::CYAN}else if device.responded {pal::BLUE}else{pal::DIM};
            painter.rect_filled(card,12.0,pal::PANEL);
            painter.rect_stroke(card,12.0,Stroke::new(if self.selected_device==Some(device.address){3.0}else{1.0},color),egui::StrokeKind::Inside);
            painter.rect_filled(Rect::from_min_size(card.min,vec2(7.0,card.height())),3.0,color);
            let cp=painter.with_clip_rect(card.shrink(10.0).intersect(ui.clip_rect()));
            let name=self.names.get(&device.address).map(|n|n.name.as_str());
            let title=name.map(str::to_string).unwrap_or_else(||device.address.to_string());
            cp.text(pos2(card.left()+16.0,card.top()+9.0),Align2::LEFT_TOP,title,theme::body(18.0),color);
            cp.text(pos2(card.left()+16.0,card.top()+34.0),Align2::LEFT_TOP,
                if name.is_some(){device.address.to_string()}else if device.address==interface.address{host.into()}else{"Name not reported".into()},theme::mono(14.0),pal::TAN);
            cp.text(pos2(card.left()+16.0,card.top()+60.0),Align2::LEFT_TOP,
                format!("{} · {} OPEN PORT{}",self.role(device.address).to_uppercase(),device.open_ports.len(),if device.open_ports.len()==1{""}else{"S"}),theme::mono(12.0),pal::DIM);
            let hint=format!("{}\n{}\n{}\n{}",name.unwrap_or("Name not reported"),device.address,device.mac.as_deref().unwrap_or("MAC unavailable"),ports_text(&device.open_ports));
            if ui.interact(card,ui.scope_id().with(device.address),Sense::click()).on_hover_text(hint).clicked(){self.select_device(device.address);}
        }
    }

    pub fn export(&self, host: &str) -> String {
        let mut text = format!(
            "Network scan from {host}\n{} · {}/{} addresses\n",
            self.status, self.completed, self.total
        );
        if let Some((interface, cidr)) = &self.surveyed {
            text.push_str(&format!(
                "Subnet {cidr}; adapter {} {}; gateway {:?}\n",
                interface.name, interface.address, interface.gateway
            ));
        }
        text.push_str(&format!(
            "Selected TCP ports: {}\n",
            self.surveyed_ports
                .iter()
                .map(u16::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ));
        text.push_str(&format!("Result filter: {}\n", if self.filter.is_empty() {"none"} else {&self.filter}));
        text.push_str("IP\tDevice name\tName source\tRole\tMAC\tDiscovery\tOpen TCP ports\n");
        for d in self.visible() {
            text.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                d.address,
                self.names.get(&d.address).map(|n| n.name.as_str()).unwrap_or(""),
                self.names.get(&d.address).map(|n| n.source.as_str()).unwrap_or(""),
                self.role(d.address),
                d.mac.as_deref().unwrap_or(""),
                if d.responded {
                    "Responded"
                } else {
                    "Neighbor cache (unconfirmed)"
                },
                ports_text(&d.open_ports)
            ));
        }
        text
    }
}

/// Web services get an instrument-style inset and explicit remote actions.
/// Other ports remain informational; no local hyperlink is created.
fn ports_ui(ui: &mut Ui, device: &NetworkDevice, enabled: bool) -> Option<(Ipv4Addr,u16)> {
    if device.open_ports.is_empty() {
        ui.label(RichText::new(ports_text(&[])).color(pal::BLUE));
        return None;
    }
    let web = device.open_ports.iter().any(|p| matches!(p,80|443));
    let frame = if web {
        egui::Frame::new().fill(egui::Color32::from_rgb(12,28,40))
            .stroke(Stroke::new(1.0,pal::BLUE.gamma_multiply(0.7))).corner_radius(6.0).inner_margin(6)
    } else { egui::Frame::NONE };
    let mut clicked = None;
    frame.show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            for &port in &device.open_ports {
                if let Ok(url) = network::web_url(device.address,port) {
                    let color = if port == 443 {pal::GREEN} else {pal::BLUE};
                    let button = egui::Button::new(RichText::new(format!("{port} {} ↗",network::service(port))).color(pal::BG))
                        .fill(color).corner_radius(6.0).min_size(vec2(0.0,28.0));
                    if ui.add_enabled(enabled,button).on_hover_text(format!("Open {url} in the REMOTE computer's default browser"))
                        .on_disabled_hover_text("Requires an active session with remote control enabled; wait for any browser request to finish").clicked() {
                        clicked = Some((device.address,port));
                    }
                } else {
                    ui.label(RichText::new(format!("{port} {}",network::service(port))).color(pal::BLUE));
                }
            }
        });
    });
    clicked
}

/// The gateway already has its own node above the survey; keep one identity.
fn topology_members(mut devices: Vec<NetworkDevice>, gateway: Option<Ipv4Addr>) -> Vec<NetworkDevice> {
    devices.retain(|d|Some(d.address)!=gateway);devices
}


fn ports_text(ports: &[u16]) -> String {
    if ports.is_empty() {
        return "None found in selected ports".into();
    }
    ports
        .iter()
        .map(|&p| format!("{p} {}", network::service(p)))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test(flavor="multi_thread")]
    async fn closing_cancels_pending_work_preserves_tunnels_and_disconnect_releases_them() {
        use slipstream_core::config::Store;
        use slipstream_session::{Client, ClientOptions, AudioOut};
        use std::time::Duration;
        let work=tempfile::tempdir().unwrap();
        let mut clients=Vec::new();
        for (name,host) in [("host",true),("viewer",false)] {
            let store=Store::open(work.path().join(name)).unwrap();
            let mut settings=store.load_settings();
            settings.server.use_public_relays=false;
            settings.host.allow_tunnels=true;
            settings.host.set_permanent_password(Some("scanner-test"));
            store.save_settings(&settings).unwrap();
            clients.push(Client::start(store,ClientOptions {
                test_pattern:host,accept_sessions:host,
                clipboard:slipstream_platform::clipboard::ClipboardSource::Memory(Default::default()),
                audio_out:AudioOut::Memory(Default::default()),
            }).await.unwrap().0);
        }
        let (session,_)=clients[1].connect_direct(clients[0].endpoint_addr(),"scanner-test",|_|{}).await.unwrap();
        let mut scanner=NetworkScanner::new(session.fs(),true,true);
        scanner.open=true;
        let tunnel=session.tunnel(0,"127.0.0.1".into(),1).await.unwrap();
        let port=tunnel.local_port;
        scanner.tunnels.push(tunnel);
        let (started,ready)=tokio::sync::oneshot::channel();
        let (held,cancelled)=tokio::sync::oneshot::channel::<()>();
        let rt=tokio::runtime::Handle::current();
        let (tx,rx)=tokio::sync::oneshot::channel();
        scanner.support_rx=Some(rx);
        scanner.support_job=Some(rt.spawn(async move {
            let _held=held;
            let _=started.send(());
            std::future::pending::<()>().await;
            let _=tx.send(Ok(ToolReply::Message(String::new())));
        }));
        ready.await.unwrap();
        scanner.hide();
        assert!(!scanner.open && scanner.support_rx.is_none());
        assert!(tokio::time::timeout(Duration::from_secs(2),cancelled).await.unwrap().is_err());
        assert!(scanner.support_notice.as_ref().unwrap().is_err());
        assert_eq!(scanner.tunnels.len(),1);
        assert!(tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST,port)).await.is_err());
        // Even a completed request must be discarded on close, so reopening
        // cannot launch its browser or keep its unreviewed new tunnel alive.
        let pending=session.tunnel(0,"127.0.0.1".into(),1).await.unwrap();
        let pending_port=pending.local_port;
        let (tx,rx)=tokio::sync::oneshot::channel();
        scanner.support_rx=Some(rx);
        assert!(tx.send(Ok(ToolReply::Tunnel(pending,80))).is_ok());
        scanner.hide();
        scanner.disconnect();
        assert!(scanner.tunnels.is_empty());
        for port in [port,pending_port] {
            tokio::time::timeout(Duration::from_secs(2),async {
                loop {
                    if tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST,port)).await.is_ok() {break;}
                    tokio::task::yield_now().await;
                }
            }).await.expect("closed tunnel retained its listener");
        }
        drop(scanner);drop(session);
        for client in clients {client.shutdown().await;}
    }
    #[test]
    fn gateway_is_not_duplicated_among_members() {
        let gateway=Ipv4Addr::new(192,168,1,1);
        let member=Ipv4Addr::new(192,168,1,2);
        let device=|address|NetworkDevice{address,mac:None,open_ports:vec![80],responded:true};
        let devices=vec![device(gateway),device(member)];
        assert_eq!(topology_members(devices.clone(),Some(gateway)),vec![device(member)]);
        assert_eq!(topology_members(devices.clone(),None),devices);
    }
}
