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
}

impl Drop for NetworkScanner {
    fn drop(&mut self) {
        if let Some(job) = self.names_job.take() { job.abort(); }
        if let Some(job) = self.adapter_job.take() {
            job.abort();
        }
    }
}

impl NetworkScanner {
    pub fn new(fs: RemoteFs) -> Self {
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
    pub fn show(shared: &Arc<Mutex<Self>>, ctx: &egui::Context, rt: &Arc<tokio::runtime::Runtime>, id: u64, host: &str, connected: bool) {
        if !shared.lock().expect("network window").open { return; }
        let (shared, rt, host) = (shared.clone(), rt.clone(), host.to_string());
        let viewport = egui::ViewportId::from_hash_of(("network-scan", id));
        let builder = egui::ViewportBuilder::default()
            .with_decorations(false)
            .with_title(format!("Network scan · {host} · Slipstream Remote"))
            .with_app_id("slipstream-remote")
            .with_icon({
                static ICON: std::sync::OnceLock<Arc<egui::IconData>> = std::sync::OnceLock::new();
                ICON.get_or_init(|| Arc::new(theme::icon(64))).clone()
            })
            .with_inner_size([1100.0,760.0]).with_min_inner_size([520.0,360.0]);
        ctx.show_viewport_deferred(viewport, builder, move |ui, _class| {
            let mut scanner = shared.lock().expect("network window");
            let ctx = ui.ctx().clone();
            scanner.tick(&ctx, &rt, connected);
            let body=theme::native_title_bar(ui,&format!("Network scan · {host}"));
            let ui=&mut ui.new_child(egui::UiBuilder::new().max_rect(body));
            if ui.input(|i|i.viewport().close_requested()) { scanner.open=false;scanner.stop();ctx.request_repaint_of(egui::ViewportId::ROOT);return; }
            egui::Frame::new().fill(pal::PANEL)
                .inner_margin(egui::Margin { left: 46, right: 14, top: 12, bottom: 12 })
                .show(ui, |ui| scanner.contents(ui, &ctx, &rt, &host, connected));
        });
    }

    fn tick(&mut self, ctx: &egui::Context, rt: &tokio::runtime::Runtime, connected: bool) {
        if !connected && (self.scan.is_some() || self.names_rx.is_some()) {
            self.stop(); self.status = "Session ended · results retained".into();
        }
        if !self.loaded && self.adapter_rx.is_none() && connected { self.load(ctx,rt); }
        self.poll();
        self.poll_names(ctx,rt,connected);
        if self.scan.is_some() || self.adapter_rx.is_some() || self.names_rx.is_some() { ctx.request_repaint_after(std::time::Duration::from_millis(100)); }
    }

    fn contents(&mut self, ui: &mut Ui, ctx: &egui::Context, rt: &tokio::runtime::Runtime, host: &str, connected: bool) {
                let chrome = theme::window_header_with_close(ui, &format!("Network scan · {host}"), pal::CYAN, pal::PEACH);
                let mut close = chrome.close;
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
                if let Some(address) = self.selected_device
                    && let Some(device) = self.devices.get(&address) {
                    ui.label(RichText::new(format!("{} · {} · {} · {}", self.names.get(&address).map(|n|n.name.as_str()).unwrap_or("Name not reported"), device.address, device.mac.as_deref().unwrap_or("MAC unavailable"), ports_text(&device.open_ports))).color(pal::TAN));
                }
                ui.add_space(5.0);
                egui::ScrollArea::both().id_salt("scan-results").auto_shrink([false, false])
                    .max_height((ui.available_height() - theme::FOOTER_SPACE - 64.0).max(100.0))
                    .show(ui, |ui| { if self.topology { self.topology(ui, host); } else { self.table(ui); } });
                ui.label(RichText::new("Device names are best effort · TCP service names follow port conventions · silent devices may be missed").small().color(pal::DIM));
                ui.label(RichText::new("Logical scan map: the remote gateway and discovered addresses; physical links are not discovered.").small().color(pal::DIM));
                chrome.footer(ui);
                close |= ui.input(|i| i.viewport().close_requested());
                if close {
                    self.open = false; self.stop();
                    if ui.ctx().viewport_id() != egui::ViewportId::ROOT { ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close); }
                    ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
                }
    }

    fn controls(
        &mut self,
        ui: &mut Ui,
        ctx: &egui::Context,
        rt: &tokio::runtime::Runtime,
        connected: bool,
    ) {
        let busy = self.scan.is_some();
        ui.add_enabled_ui(!busy && connected, |ui| {
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

    fn table(&mut self, ui: &mut Ui) {
        let devices = self.visible();
        if devices.is_empty() {
            ui.label(RichText::new("No matching devices yet").color(pal::DIM));
            return;
        }
        egui::Grid::new("network-table").num_columns(6).striped(true).spacing([20.0, 10.0]).show(ui, |ui| {
            for title in ["IP ADDRESS", "DEVICE NAME", "ROLE", "MAC ADDRESS", "DISCOVERY", "OPEN TCP PORTS"] { ui.label(RichText::new(title).font(theme::display(17.0)).color(pal::CYAN)); }
            ui.end_row();
            for device in devices {
                if ui.selectable_label(self.selected_device == Some(device.address), RichText::new(device.address.to_string()).monospace().color(pal::TAN)).clicked() { self.selected_device = Some(device.address); }
                let name = self.names.get(&device.address);
                ui.label(RichText::new(name.map(|n| n.name.as_str()).unwrap_or("—")).color(pal::TAN))
                    .on_hover_text(name.map(|n| n.source.as_str()).unwrap_or("No name was reported by DNS, NetBIOS or mDNS"));
                ui.label(RichText::new(self.role(device.address)).color(pal::LAVENDER));
                ui.label(RichText::new(device.mac.as_deref().unwrap_or("—")).monospace().color(pal::DIM));
                ui.label(RichText::new(if device.responded { "Responded" } else { "Neighbor entry" }).color(if device.responded { pal::GREEN } else { pal::GOLD }))
                    .on_hover_text(if device.responded { "Answered TCP or ICMP" } else { "In the remote neighbor cache; no TCP or ICMP answer confirmed it during this scan" });
                ui.vertical(|ui| { ui.set_max_width(330.0); ui.label(RichText::new(ports_text(&device.open_ports)).color(pal::BLUE)); });
                ui.end_row();
            }
        });
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
            if ui.interact(gateway_rect,ui.scope_id().with("gateway"),Sense::click()).on_hover_text(format!("{address}\n{name}\n{detail}")).clicked() { self.selected_device=Some(address); }
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
            if ui.interact(card,ui.scope_id().with(device.address),Sense::click()).on_hover_text(hint).clicked(){self.selected_device=Some(device.address);}
        }
    }

    fn export(&self, host: &str) -> String {
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
