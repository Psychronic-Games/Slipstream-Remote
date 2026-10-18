//! What a computer is: OS, processor, memory, graphics, board, drives,
//! temperatures and network, worded for the viewer's System information
//! window. Slow-ish (it samples CPU use over a moment): call it off the
//! async threads.

use slipstream_core::proto::InfoSection;
use sysinfo::{Components, CpuRefreshKind, Disks, MemoryRefreshKind, Networks, RefreshKind, System};

pub fn collect() -> Vec<InfoSection> {
    let mut sys = System::new_with_specifics(
        RefreshKind::nothing().with_cpu(CpuRefreshKind::everything()).with_memory(MemoryRefreshKind::everything()),
    );
    // CPU use is the change between two samples.
    std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL.max(std::time::Duration::from_millis(250)));
    sys.refresh_cpu_all();
    // The hardware doesn't change while we run; the viewer asks every few
    // seconds for the live figures.
    static HARDWARE: std::sync::OnceLock<Hardware> = std::sync::OnceLock::new();
    let hw = HARDWARE.get_or_init(hardware);
    let mut out = Vec::new();

    let mut computer = Section::new("Computer");
    computer.row("Name", System::host_name());
    match hypervisor(hw) {
        // A virtual machine's firmware describes a board its hypervisor
        // makes up (VMware's is a 1998 Intel 440BX); the real one is the
        // host's, which can't be seen from inside.
        Some(vm) => {
            computer.row("Model", Some(format!("{vm} virtual machine")));
            computer.row(
                "Motherboard",
                Some(format!(
                    "virtual{} (the physical computer's board can't be seen from inside a virtual machine)",
                    hw.board.as_ref().map(|b| format!(": {b}")).unwrap_or_default()
                )),
            );
        }
        None => {
            computer.row("Model", hw.system.clone());
            computer.row("Motherboard", hw.board.clone());
        }
    }
    computer.row("BIOS / firmware", hw.bios.clone());
    computer.row("Up for", Some(duration(System::uptime())));
    computer.row("User", std::env::var("USERNAME").or_else(|_| std::env::var("USER")).ok());
    out.push(computer.done());

    let mut os = Section::new("Operating system");
    os.row("System", System::long_os_version().or_else(System::name));
    os.row("Version", hw.os_build.clone().or_else(System::os_version));
    os.row("Kernel", System::kernel_version());
    os.row("Architecture", Some(System::cpu_arch()));
    out.push(os.done());

    let mut cpu = Section::new("Processor");
    let cpus = sys.cpus();
    cpu.row("Model", cpus.first().map(|c| c.brand().trim().to_string()).filter(|b| !b.is_empty()));
    let threads = cpus.len();
    cpu.row(
        "Cores",
        Some(match System::physical_core_count() {
            Some(cores) if cores != threads => format!("{cores} cores, {threads} threads"),
            _ => format!("{threads} cores"),
        }),
    );
    cpu.row("Speed", cpus.first().map(|c| c.frequency()).filter(|&f| f > 0).map(|f| format!("{:.2} GHz", f as f64 / 1000.0)));
    cpu.row("In use", Some(format!("{:.0} %", sys.global_cpu_usage())));
    out.push(cpu.done());

    let mut mem = Section::new("Memory");
    mem.row("RAM", Some(used_of(sys.used_memory(), sys.total_memory())));
    if sys.total_swap() > 0 {
        mem.row(if cfg!(windows) { "Page file" } else { "Swap" }, Some(used_of(sys.used_swap(), sys.total_swap())));
    }
    out.push(mem.done());

    let nvidia = nvidia_smi();
    let mut gpus = hw.gpus.clone();
    // nvidia-smi knows NVIDIA cards' memory, which the OS often doesn't report.
    for (name, mem, _) in &nvidia {
        let short = name.trim_start_matches("NVIDIA ");
        if let Some(g) = gpus.iter_mut().find(|g| g.contains(short)) {
            let driver = g.split_once("driver").map(|(_, d)| format!(", driver{}", d.trim_end_matches(')'))).unwrap_or_default();
            *g = format!("{name} ({mem}{driver})");
        } else {
            gpus.push(format!("{name} ({mem})"));
        }
    }
    let mut gpu = Section::new("Graphics");
    if gpus.is_empty() {
        gpu.row("Adapter", Some("not found".into()));
    }
    for (i, g) in gpus.iter().enumerate() {
        gpu.row(if gpus.len() > 1 { format!("Adapter {}", i + 1) } else { "Adapter".into() }, Some(g.clone()));
    }
    out.push(gpu.done());

    let mut storage = Section::new("Storage");
    for (i, (model, size)) in hw.drives.iter().enumerate() {
        storage.row(format!("Drive {}", i + 1), Some(format!("{model} · {}", bytes(*size))));
    }
    let disks = Disks::new_with_refreshed_list();
    let mut seen = Vec::new();
    for d in disks.list() {
        let mount = d.mount_point().display().to_string();
        // Linux lists bind mounts and snapshots of the same volume many times.
        let key = (d.name().to_os_string(), d.total_space());
        if d.total_space() == 0 || seen.contains(&key) || (cfg!(target_os = "linux") && !linux_interesting_mount(&mount)) {
            continue;
        }
        seen.push(key);
        let kind = match d.kind() {
            sysinfo::DiskKind::SSD => " SSD",
            sysinfo::DiskKind::HDD => " HDD",
            _ => "",
        };
        let removable = if d.is_removable() { ", removable" } else { "" };
        storage.row(
            mount,
            Some(format!(
                "{} free of {} ({}{kind}{removable})",
                bytes(d.available_space()),
                bytes(d.total_space()),
                d.file_system().to_string_lossy()
            )),
        );
    }
    #[cfg(windows)]
    if storage.rows.is_empty() {
        for (root, free, total, fs) in windows_volumes() {
            storage.row(root, Some(format!("{} free of {} ({fs})", bytes(free), bytes(total))));
        }
    }
    out.push(storage.done());

    let mut temps = Section::new("Temperatures");
    let mut readings: Vec<(u8, String, f32)> = Components::new_with_refreshed_list()
        .list()
        .iter()
        .filter_map(|c| {
            let t = c.temperature().filter(|t| t.is_finite() && *t > 0.0 && *t < 150.0)?;
            let (rank, name) = sensor_name(c.label())?;
            Some((rank, name, t))
        })
        .collect();
    readings.extend(nvidia.iter().filter_map(|(name, _, t)| Some((1, format!("GPU ({name})"), (*t)?))));
    readings.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    readings.dedup_by(|a, b| a.1 == b.1);
    for (_, name, t) in readings {
        temps.row(name, Some(format!("{t:.0} °C")));
    }
    if temps.rows.is_empty() {
        temps.row(
            "Sensors",
            Some(if cfg!(windows) {
                "not available (Windows shows them only to administrators and the Slipstream service)".into()
            } else {
                "not available on this computer".into()
            }),
        );
    }
    out.push(temps.done());

    // Each adapter with an address, then its details indented under it
    // (labels starting with spaces).
    let mut net = Section::new("Network");
    let networks = Networks::new_with_refreshed_list();
    let routes = net_extras();
    let mut names: Vec<_> = networks.keys().collect();
    names.sort();
    let mut gateways_shown = Vec::new();
    for name in names {
        let data = &networks[name];
        let addrs: Vec<_> = data.ip_networks().iter().filter(|n| !n.addr.is_loopback()).collect();
        let link_local = |a: &std::net::IpAddr| matches!(a, std::net::IpAddr::V6(v6) if v6.segments()[0] & 0xffc0 == 0xfe80);
        // An adapter with only a link-local address isn't connected to anything.
        if addrs.iter().all(|n| link_local(&n.addr)) {
            continue;
        }
        let mac = data.mac_address().to_string();
        net.row(name.clone(), Some(if mac == "00:00:00:00:00:00" { "connected".into() } else { format!("MAC {mac}") }));
        for n in &addrs {
            if let std::net::IpAddr::V4(a) = n.addr {
                net.row("    IPv4 address", Some(a.to_string()));
                net.row("    Subnet mask", Some(format!("{} (/{})", v4_mask(n.prefix), n.prefix)));
            }
        }
        for n in &addrs {
            if let std::net::IpAddr::V6(a) = n.addr {
                let kind = if link_local(&n.addr) { " (link-local)" } else { "" };
                net.row("    IPv6 address", Some(format!("{a}/{}{kind}", n.prefix)));
            }
        }
        for (dev, gw) in &routes.gateways {
            if dev == name {
                net.row("    Gateway", Some(gw.to_string()));
                gateways_shown.push(*gw);
            }
        }
    }
    let gateways: Vec<String> = routes
        .gateways
        .iter()
        .filter(|(_, gw)| !gateways_shown.contains(gw))
        .map(|(dev, gw)| if dev.is_empty() { gw.to_string() } else { format!("{gw} on {dev}") })
        .collect();
    net.row(if gateways.len() > 1 { "Gateways" } else { "Gateway" }, Some(gateways.join(", ")).filter(|g| !g.is_empty()));
    let dns: Vec<String> = routes.dns.iter().map(ToString::to_string).collect();
    net.row("DNS servers", Some(dns.join(", ")).filter(|d| !d.is_empty()));
    out.push(net.done());

    out.retain(|s| !s.rows.is_empty());
    out
}

/// The hypervisor's name, if this computer is a virtual machine (going by
/// what its firmware calls the system and board).
fn hypervisor(hw: &Hardware) -> Option<&'static str> {
    let says = format!("{} {}", hw.system.as_deref().unwrap_or(""), hw.board.as_deref().unwrap_or("")).to_lowercase();
    const KNOWN: [(&str, &str); 9] = [
        ("vmware", "VMware"),
        ("440bx desktop reference platform", "VMware"),
        ("virtualbox", "VirtualBox"),
        ("innotek", "VirtualBox"),
        ("microsoft corporation virtual machine", "Hyper-V"),
        ("qemu", "QEMU/KVM"),
        ("kvm", "QEMU/KVM"),
        ("xen", "Xen"),
        ("parallels", "Parallels"),
    ];
    KNOWN.iter().find(|(k, _)| says.contains(k)).map(|(_, name)| *name)
}

/// "255.255.255.0" for a /24.
fn v4_mask(prefix: u8) -> std::net::Ipv4Addr {
    let bits = if prefix == 0 { 0 } else { u32::MAX << (32 - u32::from(prefix.min(32))) };
    std::net::Ipv4Addr::from(bits)
}

/// Default gateways (with their interface) and DNS servers.
#[derive(Default)]
struct NetExtras {
    gateways: Vec<(String, std::net::IpAddr)>,
    dns: Vec<std::net::IpAddr>,
}

impl NetExtras {
    fn add_gateway(&mut self, dev: String, gw: std::net::IpAddr) {
        if !gw.is_unspecified() && !self.gateways.iter().any(|(_, g)| *g == gw) {
            self.gateways.push((dev, gw));
        }
    }
    fn add_dns(&mut self, ip: std::net::IpAddr) {
        if !self.dns.contains(&ip) {
            self.dns.push(ip);
        }
    }
}

#[cfg(windows)]
fn net_extras() -> NetExtras {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    use windows_sys::Win32::{
        NetworkManagement::IpHelper::{
            GAA_FLAG_INCLUDE_GATEWAYS, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_MULTICAST, GetAdaptersAddresses, IP_ADAPTER_ADDRESSES_LH,
        },
        Networking::WinSock::{AF_INET, AF_INET6, AF_UNSPEC, SOCKADDR, SOCKADDR_IN, SOCKADDR_IN6},
    };
    const ERROR_BUFFER_OVERFLOW: u32 = 111;
    const IF_OPER_STATUS_UP: i32 = 1;
    const IF_TYPE_SOFTWARE_LOOPBACK: u32 = 24;
    // SAFETY: a sockaddr from GetAdaptersAddresses, read by its family.
    let ip = |sa: *const SOCKADDR| unsafe {
        if sa.is_null() {
            return None;
        }
        match (*sa).sa_family {
            AF_INET => Some(IpAddr::V4(Ipv4Addr::from((*(sa as *const SOCKADDR_IN)).sin_addr.S_un.S_addr.to_ne_bytes()))),
            AF_INET6 => Some(IpAddr::V6(Ipv6Addr::from((*(sa as *const SOCKADDR_IN6)).sin6_addr.u.Byte))),
            _ => None,
        }
    };
    let mut out = NetExtras::default();
    let flags = GAA_FLAG_INCLUDE_GATEWAYS | GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST;
    // u64s: the adapter structs need 8-byte alignment.
    let mut buf = vec![0u64; 4096];
    // SAFETY: GetAdaptersAddresses fills `buf` (asking again with the size it
    // wants if too small); the lists inside it are walked read-only.
    unsafe {
        for _ in 0..3 {
            let mut len = (buf.len() * 8) as u32;
            match GetAdaptersAddresses(AF_UNSPEC as u32, flags, std::ptr::null(), buf.as_mut_ptr().cast(), &mut len) {
                0 => break,
                ERROR_BUFFER_OVERFLOW => buf = vec![0u64; len as usize / 8 + 1],
                _ => return out,
            }
        }
        let mut a = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
        while !a.is_null() {
            let ad = &*a;
            if ad.OperStatus == IF_OPER_STATUS_UP && ad.IfType != IF_TYPE_SOFTWARE_LOOPBACK {
                let name = if ad.FriendlyName.is_null() {
                    String::new()
                } else {
                    let n = (0..).take_while(|&i| *ad.FriendlyName.add(i) != 0).count();
                    String::from_utf16_lossy(std::slice::from_raw_parts(ad.FriendlyName, n))
                };
                let mut g = ad.FirstGatewayAddress;
                while !g.is_null() {
                    if let Some(addr) = ip((*g).Address.lpSockaddr) {
                        out.add_gateway(name.clone(), addr);
                    }
                    g = (*g).Next;
                }
                let mut d = ad.FirstDnsServerAddress;
                while !d.is_null() {
                    // Site-local fec0:0:0:ffff::* are placeholders Windows lists when there's no IPv6 DNS.
                    if let Some(addr) = ip((*d).Address.lpSockaddr).filter(|a| !matches!(a, IpAddr::V6(v6) if v6.segments()[0] == 0xfec0)) {
                        out.add_dns(addr);
                    }
                    d = (*d).Next;
                }
            }
            a = ad.Next;
        }
    }
    out
}

#[cfg(target_os = "linux")]
fn net_extras() -> NetExtras {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    let mut out = NetExtras::default();
    // Default routes: Destination 00000000 with RTF_GATEWAY; addresses are
    // little-endian hex.
    for line in std::fs::read_to_string("/proc/net/route").unwrap_or_default().lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() > 3
            && f[1] == "00000000"
            && let (Ok(gw), Ok(flags)) = (u32::from_str_radix(f[2], 16), u32::from_str_radix(f[3], 16))
            && flags & 2 != 0
        {
            out.add_gateway(f[0].to_string(), IpAddr::V4(Ipv4Addr::from(gw.to_le_bytes())));
        }
    }
    // dest(32 hex) dest_len src src_len next_hop metric refcnt use flags iface
    for line in std::fs::read_to_string("/proc/net/ipv6_route").unwrap_or_default().lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() >= 10
            && f[0].chars().all(|c| c == '0')
            && f[1] == "00"
            && !f[4].chars().all(|c| c == '0')
            && f[9] != "lo"
            && let Ok(v) = u128::from_str_radix(f[4], 16)
        {
            out.add_gateway(f[9].to_string(), IpAddr::V6(Ipv6Addr::from(v)));
        }
    }
    // systemd-resolved's real upstream servers, not its 127.0.0.53 stub.
    let conf = std::fs::read_to_string("/run/systemd/resolve/resolv.conf").or_else(|_| std::fs::read_to_string("/etc/resolv.conf")).unwrap_or_default();
    for line in conf.lines() {
        if let Some(ip) = line.trim().strip_prefix("nameserver").and_then(|r| r.trim().split('%').next()?.parse().ok()) {
            out.add_dns(ip);
        }
    }
    out
}

#[cfg(target_os = "macos")]
fn net_extras() -> NetExtras {
    let run = |cmd: &str, args: &[&str]| {
        std::process::Command::new(cmd).args(args).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
    };
    let mut out = NetExtras::default();
    let route = run("route", &["-n", "get", "default"]);
    let field = |k: &str| route.lines().find_map(|l| l.trim().strip_prefix(k).map(|v| v.trim().to_string()));
    if let Some(gw) = field("gateway:").and_then(|g| g.parse().ok()) {
        out.add_gateway(field("interface:").unwrap_or_default(), gw);
    }
    for line in run("scutil", &["--dns"]).lines() {
        if let Some(ip) = line.trim().strip_prefix("nameserver[").and_then(|r| r.split_once(':')).and_then(|(_, v)| v.trim().parse().ok()) {
            out.add_dns(ip);
        }
    }
    out
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn net_extras() -> NetExtras {
    NetExtras::default()
}

/// Builds one section, leaving out facts this computer doesn't report.
struct Section(InfoSection);

impl Section {
    fn new(title: &str) -> Self {
        Self(InfoSection { title: title.into(), rows: Vec::new() })
    }
    fn row(&mut self, label: impl Into<String>, value: Option<String>) {
        if let Some(v) = value.map(|v| v.trim().to_string()).filter(|v| !v.is_empty()) {
            self.0.rows.push((label.into(), v));
        }
    }
    fn done(self) -> InfoSection {
        self.0
    }
}

impl std::ops::Deref for Section {
    type Target = InfoSection;
    fn deref(&self) -> &InfoSection {
        &self.0
    }
}

fn bytes(n: u64) -> String {
    const GB: f64 = (1u64 << 30) as f64;
    let g = n as f64 / GB;
    if g >= 1000.0 {
        format!("{:.1} TB", g / 1024.0)
    } else if g >= 10.0 {
        format!("{g:.0} GB")
    } else if g >= 1.0 {
        format!("{g:.1} GB")
    } else {
        format!("{:.0} MB", n as f64 / (1u64 << 20) as f64)
    }
}

fn used_of(used: u64, total: u64) -> String {
    let pct = (used * 100).checked_div(total).unwrap_or(0);
    format!("{} used of {} ({pct} %)", bytes(used), bytes(total))
}

fn duration(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, secs / 3600 % 24, secs / 60 % 60);
    match (d, h) {
        (0, 0) => format!("{m} min"),
        (0, _) => format!("{h} h {m} min"),
        _ => format!("{d} d {h} h"),
    }
}

#[cfg(target_os = "linux")]
fn linux_interesting_mount(mount: &str) -> bool {
    !["/boot", "/snap", "/var/lib", "/run", "/sys", "/proc", "/dev"].iter().any(|p| mount.starts_with(p))
}

#[cfg(not(target_os = "linux"))]
fn linux_interesting_mount(_: &str) -> bool {
    true
}

/// A readable name for a temperature sensor, and where it sorts (CPU,
/// GPU, drives, the rest); `None` for redundant ones.
fn sensor_name(label: &str) -> Option<(u8, String)> {
    let (chip, rest) = label.split_once(' ').unwrap_or((label, ""));
    let out = match chip {
        "k10temp" | "coretemp" | "zenpower" => match rest {
            "Tctl" | "Tdie" | "Package id 0" => (0, "CPU".to_string()),
            r if r.starts_with("Tccd") => (0, format!("CPU die {}", &r[4..])),
            r if r.starts_with("Core ") => return None, // one per core: the package says enough
            r => (0, format!("CPU ({r})")),
        },
        "amdgpu" | "radeon" | "nouveau" | "i915" | "xe" => match rest {
            "edge" | "temp1" | "" => (1, format!("GPU ({chip})")),
            "junction" => (1, format!("GPU hotspot ({chip})")),
            "mem" => (1, format!("GPU memory ({chip})")),
            r => (1, format!("GPU {r} ({chip})")),
        },
        // The composite reading only, not the per-sensor ones beside it.
        "nvme" => (2, rest.strip_prefix("Composite ")?.to_string()),
        "spd5118" | "jc42" => (3, "Memory module".to_string()),
        "acpitz" => (3, "Motherboard (ACPI)".to_string()),
        c if c.contains("xhci") => (3, "USB controller".to_string()),
        c if ["r8169", "r8125", "igb", "igc", "e1000e", "ixgbe", "iwlwifi", "mt79", "ath1"].iter().any(|n| c.starts_with(n)) => {
            (3, "Network adapter".to_string())
        }
        _ if rest.starts_with("temp") && rest.len() <= 5 => (4, chip.replace('_', " ")),
        _ => (4, label.to_string()),
    };
    Some(out)
}

/// NVIDIA cards via nvidia-smi, when the driver installed it: (name,
/// memory, temperature).
fn nvidia_smi() -> Vec<(String, String, Option<f32>)> {
    let mut cmd = std::process::Command::new("nvidia-smi");
    cmd.args(["--query-gpu=name,memory.total,temperature.gpu", "--format=csv,noheader,nounits"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let Ok(out) = cmd.output() else { return Vec::new() };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(',').map(str::trim).collect();
            let name = f.first().filter(|n| !n.is_empty())?.to_string();
            let mem = f.get(1).and_then(|m| m.parse::<u64>().ok()).map(|mib| bytes(mib << 20)).unwrap_or_default();
            Some((name, mem, f.get(2).and_then(|t| t.parse().ok())))
        })
        .collect()
}

/// What `sysinfo` doesn't cover, per OS.
#[derive(Default)]
struct Hardware {
    system: Option<String>,
    board: Option<String>,
    bios: Option<String>,
    os_build: Option<String>,
    gpus: Vec<String>,
    /// Physical drives: (model, size).
    drives: Vec<(String, u64)>,
}

/// "Vendor Product", leaving out placeholder strings firmware is full of.
#[cfg(any(windows, target_os = "linux"))]
fn join_names(parts: &[Option<String>]) -> Option<String> {
    const JUNK: [&str; 6] = ["to be filled by o.e.m.", "default string", "system product name", "system manufacturer", "not applicable", "o.e.m."];
    let words: Vec<String> = parts
        .iter()
        .flatten()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && !JUNK.contains(&s.to_lowercase().as_str()))
        .collect();
    (!words.is_empty()).then(|| words.join(" "))
}

#[cfg(windows)]
fn hardware() -> Hardware {
    use windows_sys::Win32::System::Registry::HKEY_LOCAL_MACHINE;
    let bios = r"HARDWARE\DESCRIPTION\System\BIOS";
    let nt = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    let get = |key: &str, value: &str| reg_string(HKEY_LOCAL_MACHINE, key, value);
    let build = match (get(nt, "DisplayVersion").or_else(|| get(nt, "ReleaseId")), get(nt, "CurrentBuild")) {
        (Some(v), Some(b)) => Some(format!("{v} (build {b}{})", reg_dword(HKEY_LOCAL_MACHINE, nt, "UBR").map(|u| format!(".{u}")).unwrap_or_default())),
        (None, Some(b)) => Some(format!("build {b}")),
        _ => None,
    };
    // Display adapters: the device class's numbered driver keys.
    let class = r"SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}";
    let mut gpus = Vec::new();
    for i in 0..16 {
        let key = format!(r"{class}\{i:04}");
        let Some(name) = get(&key, "DriverDesc") else { continue };
        let vram = reg_qword(HKEY_LOCAL_MACHINE, &key, "HardwareInformation.qwMemorySize")
            .or_else(|| reg_dword(HKEY_LOCAL_MACHINE, &key, "HardwareInformation.MemorySize").map(u64::from))
            .filter(|&b| b >= 64 << 20); // integrated graphics report a token amount
        let driver = get(&key, "DriverVersion").map(|v| format!(", driver {v}")).unwrap_or_default();
        let line = match vram {
            Some(b) => format!("{name} ({}{driver})", bytes(b)),
            None if driver.is_empty() => name,
            None => format!("{name} ({})", driver.trim_start_matches(", ")),
        };
        if !gpus.contains(&line) {
            gpus.push(line);
        }
    }
    Hardware {
        system: join_names(&[get(bios, "SystemManufacturer"), get(bios, "SystemProductName")]),
        board: join_names(&[get(bios, "BaseBoardManufacturer"), get(bios, "BaseBoardProduct")]),
        bios: join_names(&[get(bios, "BIOSVendor"), get(bios, "BIOSVersion"), get(bios, "BIOSReleaseDate")]),
        os_build: build,
        gpus,
        drives: Vec::new(),
    }
}

/// Drive letters with their space and file system, straight from Windows
/// (for when `sysinfo` lists none).
#[cfg(windows)]
fn windows_volumes() -> Vec<(String, u64, u64, String)> {
    use windows_sys::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW};
    const DRIVE_FIXED: u32 = 3;
    const DRIVE_REMOVABLE: u32 = 2;
    let mut out = Vec::new();
    // SAFETY: plain queries with NUL-terminated paths and sized buffers.
    unsafe {
        let mask = GetLogicalDrives();
        for i in 0..26u8 {
            if mask & (1 << i) == 0 {
                continue;
            }
            let root = format!("{}:\\", (b'A' + i) as char);
            let w: Vec<u16> = root.encode_utf16().chain([0]).collect();
            if !matches!(GetDriveTypeW(w.as_ptr()), DRIVE_FIXED | DRIVE_REMOVABLE) {
                continue;
            }
            let (mut free, mut total) = (0u64, 0u64);
            if GetDiskFreeSpaceExW(w.as_ptr(), &mut free, &mut total, std::ptr::null_mut()) == 0 || total == 0 {
                continue;
            }
            let mut fs = [0u16; 32];
            let fs = if GetVolumeInformationW(w.as_ptr(), std::ptr::null_mut(), 0, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), fs.as_mut_ptr(), fs.len() as u32) != 0 {
                String::from_utf16_lossy(&fs[..fs.iter().position(|&c| c == 0).unwrap_or(0)])
            } else {
                String::new()
            };
            out.push((root, free, total, fs));
        }
    }
    out
}

#[cfg(windows)]
fn reg_raw(root: windows_sys::Win32::System::Registry::HKEY, key: &str, value: &str, flags: u32) -> Option<Vec<u8>> {
    use windows_sys::Win32::System::Registry::RegGetValueW;
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (k, v) = (wide(key), wide(value));
    let mut len = 0u32;
    // SAFETY: sizing call, then a read into a buffer of that size.
    unsafe {
        if RegGetValueW(root, k.as_ptr(), v.as_ptr(), flags, std::ptr::null_mut(), std::ptr::null_mut(), &mut len) != 0 || len == 0 {
            return None;
        }
        let mut buf = vec![0u8; len as usize];
        if RegGetValueW(root, k.as_ptr(), v.as_ptr(), flags, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut len) != 0 {
            return None;
        }
        buf.truncate(len as usize);
        Some(buf)
    }
}

#[cfg(windows)]
fn reg_string(root: windows_sys::Win32::System::Registry::HKEY, key: &str, value: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::RRF_RT_REG_SZ;
    let buf = reg_raw(root, key, value, RRF_RT_REG_SZ)?;
    let units: Vec<u16> = buf.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).take_while(|&u| u != 0).collect();
    Some(String::from_utf16_lossy(&units)).filter(|s| !s.trim().is_empty())
}

#[cfg(windows)]
fn reg_dword(root: windows_sys::Win32::System::Registry::HKEY, key: &str, value: &str) -> Option<u32> {
    use windows_sys::Win32::System::Registry::RRF_RT_REG_DWORD;
    let buf = reg_raw(root, key, value, RRF_RT_REG_DWORD)?;
    Some(u32::from_le_bytes(buf.get(..4)?.try_into().ok()?))
}

#[cfg(windows)]
fn reg_qword(root: windows_sys::Win32::System::Registry::HKEY, key: &str, value: &str) -> Option<u64> {
    use windows_sys::Win32::System::Registry::RRF_RT_REG_QWORD;
    let buf = reg_raw(root, key, value, RRF_RT_REG_QWORD)?;
    Some(u64::from_le_bytes(buf.get(..8)?.try_into().ok()?))
}

#[cfg(target_os = "linux")]
fn hardware() -> Hardware {
    let dmi = |f: &str| std::fs::read_to_string(format!("/sys/class/dmi/id/{f}")).ok().map(|s| s.trim().to_string());
    let mut gpus = Vec::new();
    let mut cards: Vec<_> = std::fs::read_dir("/sys/class/drm")
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("card") && !n.contains('-'))
        .collect();
    cards.sort();
    for card in cards {
        let dev = format!("/sys/class/drm/{card}/device");
        let uevent = std::fs::read_to_string(format!("{dev}/uevent")).unwrap_or_default();
        let field = |k: &str| uevent.lines().find_map(|l| l.strip_prefix(k)).map(str::to_string);
        let slot = field("PCI_SLOT_NAME=");
        let name = slot.as_deref().and_then(lspci_name).or_else(|| field("PCI_ID=").map(|id| format!("PCI device {id}")));
        let Some(name) = name else { continue };
        let vram = std::fs::read_to_string(format!("{dev}/mem_info_vram_total")).ok().and_then(|s| s.trim().parse::<u64>().ok());
        let driver = field("DRIVER=").map(|d| format!("driver {d}"));
        let extra: Vec<String> = vram.map(bytes).into_iter().chain(driver).collect();
        let line = if extra.is_empty() { name } else { format!("{name} ({})", extra.join(", ")) };
        if !gpus.contains(&line) {
            gpus.push(line);
        }
    }
    Hardware {
        system: join_names(&[dmi("sys_vendor"), dmi("product_name")]),
        board: join_names(&[dmi("board_vendor"), dmi("board_name")]),
        bios: join_names(&[dmi("bios_vendor"), dmi("bios_version"), dmi("bios_date")]),
        os_build: None,
        gpus,
        drives: linux_drives(),
    }
}

/// Physical disks from /sys/block (not partitions, loop or RAM devices).
#[cfg(target_os = "linux")]
fn linux_drives() -> Vec<(String, u64)> {
    let mut names: Vec<String> = std::fs::read_dir("/sys/block")
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !["loop", "ram", "zram", "dm-", "sr", "md"].iter().any(|p| n.starts_with(p)))
        .collect();
    names.sort();
    names
        .into_iter()
        .filter_map(|n| {
            let read = |f: &str| std::fs::read_to_string(format!("/sys/block/{n}/{f}")).ok().map(|s| s.trim().to_string());
            let sectors: u64 = read("size")?.parse().ok()?;
            let model = read("device/model").filter(|m| !m.is_empty()).unwrap_or_else(|| n.clone());
            (sectors > 0).then_some((model, sectors * 512))
        })
        .collect()
}

/// "Advanced Micro Devices, Inc. [AMD/ATI] Navi 31 [Radeon RX 7900 XTX]",
/// when pciutils is installed.
#[cfg(target_os = "linux")]
fn lspci_name(slot: &str) -> Option<String> {
    let out = std::process::Command::new("lspci").args(["-mm", "-s", slot]).output().ok()?;
    let line = String::from_utf8_lossy(&out.stdout).into_owned();
    // slot "class" "vendor" "device" …
    let fields: Vec<&str> = line.split('"').filter(|s| !s.trim().is_empty()).collect();
    (fields.len() >= 4).then(|| format!("{} {}", fields[2], fields[3]))
}

#[cfg(target_os = "macos")]
fn hardware() -> Hardware {
    let run = |cmd: &str, args: &[&str]| {
        std::process::Command::new(cmd).args(args).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let gpus = run("system_profiler", &["SPDisplaysDataType"])
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.trim().strip_prefix("Chipset Model:").map(|s| s.trim().to_string()))
        .collect();
    Hardware {
        system: run("sysctl", &["-n", "hw.model"]).map(|m| format!("Apple {m}")),
        board: None,
        bios: None,
        os_build: run("sw_vers", &["-buildVersion"]).map(|b| format!("build {b}")),
        gpus,
        drives: Vec::new(),
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn hardware() -> Hardware {
    Hardware::default()
}

#[cfg(test)]
mod tests {
    #[test]
    fn collects_the_basics() {
        let info = super::collect();
        let titles: Vec<&str> = info.iter().map(|s| s.title.as_str()).collect();
        for t in ["Computer", "Operating system", "Processor", "Memory"] {
            assert!(titles.contains(&t), "{t} missing from {titles:?}");
        }
        for s in &info {
            println!("{}", s.title);
            for (k, v) in &s.rows {
                println!("  {k}: {v}");
            }
        }
    }

    #[test]
    fn spots_virtual_machines() {
        let hw = |system: &str, board: &str| super::Hardware { system: Some(system.into()), board: Some(board.into()), ..Default::default() };
        assert_eq!(super::hypervisor(&hw("VMware, Inc. VMware Virtual Platform", "Intel Corporation 440BX Desktop Reference Platform")), Some("VMware"));
        assert_eq!(super::hypervisor(&hw("Microsoft Corporation Virtual Machine", "Microsoft Corporation Virtual Machine")), Some("Hyper-V"));
        assert_eq!(super::hypervisor(&hw("Supermicro SYS-5019C-M", "Supermicro X11SCM-F")), None);
    }

    #[test]
    fn sizes_read_well() {
        assert_eq!(super::bytes(16 << 30), "16 GB");
        assert_eq!(super::bytes(3 << 29), "1.5 GB");
        assert_eq!(super::bytes(2 << 40), "2.0 TB");
        assert_eq!(super::duration(3 * 86_400 + 5 * 3600), "3 d 5 h");
    }
}
