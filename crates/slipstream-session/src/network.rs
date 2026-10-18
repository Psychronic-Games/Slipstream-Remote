//! Discovery runs on the remote host, on a separate, low-priority stream.
//! No raw sockets or administrator rights are required. TCP refusal proves
//! reachability too; ICMP and neighbor entries find devices without open ports.

use crate::RemoteFs;
use anyhow::{Result, bail};
use iroh::endpoint::SendStream;
use slipstream_core::{
    framing::{expect_msg, write_msg},
    network::{NetworkDevice, NetworkInterface, ScanEvent, ScanRequest, validate},
    proto::{StreamOpen, StreamReply},
};
use std::{
    collections::BTreeMap,
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio::{
    net::TcpSocket,
    sync::{Semaphore, mpsc},
    task::JoinSet,
};

/// Dropping this handle (Stop or closing the tool) cancels the QUIC request;
/// the host drops its outstanding probes and any ping subprocesses with it.
pub struct RemoteScan {
    pub events: mpsc::Receiver<Result<ScanEvent, String>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for RemoteScan {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl RemoteFs {
    pub async fn network_interfaces(&self) -> Result<Vec<NetworkInterface>> {
        match self.ask(StreamOpen::NetworkInterfaces).await?.2 {
            StreamReply::NetworkInterfaces(i) => Ok(i),
            other => bail!("Unexpected network adapter reply: {other:?}"),
        }
    }

    /// Start receiving a survey. Call from a Tokio runtime context.
    pub fn network_scan(&self, request: ScanRequest) -> RemoteScan {
        let fs = self.clone();
        let (tx, events) = mpsc::channel(64);
        let task = tokio::spawn(async move {
            let r = async {
                let (_send, mut recv, first) = tokio::time::timeout(
                    Duration::from_secs(10),
                    fs.ask(StreamOpen::NetworkScan(request)),
                )
                .await
                .map_err(|_| {
                    anyhow::anyhow!("The remote computer did not answer the scan request")
                })??;
                let mut reply = first;
                loop {
                    match reply {
                        StreamReply::NetworkScan(event) => {
                            let finished = matches!(event, ScanEvent::Finished);
                            if tx.send(Ok(event)).await.is_err() || finished {
                                return Ok(());
                            }
                        }
                        StreamReply::Error(e) => bail!("{e}"),
                        other => bail!("Unexpected scan reply: {other:?}"),
                    }
                    reply = tokio::time::timeout(
                        Duration::from_secs(90),
                        expect_msg::<StreamReply>(&mut recv),
                    )
                    .await
                    .map_err(|_| anyhow::anyhow!("The remote scan stopped reporting progress"))??;
                }
            }
            .await;
            if let Err(e) = r {
                let _ = tx.send(Err(format!("Network scan: {e:#}"))).await;
            }
        });
        RemoteScan { events, task }
    }
}

/// Limit all scans on this host together, including concurrent viewers.
static PROBES: std::sync::OnceLock<Arc<Semaphore>> = std::sync::OnceLock::new();

pub(crate) async fn serve(
    send: &mut SendStream,
    request: ScanRequest,
    interfaces: Vec<NetworkInterface>,
) -> Result<()> {
    let (first, last) = match validate(&request, &interfaces) {
        Ok(range) => range,
        Err(e) => return write_msg(send, &StreamReply::Error(e.to_string())).await,
    };
    let total = last - first + 1;
    let event = |e| StreamReply::NetworkScan(e);
    write_msg(send, &event(ScanEvent::Started { total })).await?;
    let mut pending = first..=last;
    let ports = Arc::new(request.ports.clone());
    let mut jobs = JoinSet::new();
    let mut found = BTreeMap::new();
    let mut completed = 0;
    loop {
        while jobs.len() < 16 {
            let Some(address) = pending.next() else { break };
            let ports = ports.clone();
            let source = request.interface;
            jobs.spawn(async move { probe(Ipv4Addr::from(address), source, ports).await });
        }
        let Some(device) = jobs.join_next().await else {
            break;
        };
        let device = device?;
        if device.responded || device.address == request.interface {
            found.insert(device.address, device.clone());
            write_msg(send, &event(ScanEvent::Device(device))).await?;
        }
        completed += 1;
        write_msg(send, &event(ScanEvent::Progress { completed, total })).await?;
    }
    // Read after probing: normal TCP/ICMP traffic populated the host's ARP
    // table, even for devices that silently filter every selected TCP port.
    let interface = interfaces
        .iter()
        .find(|i| i.address == request.interface)
        .expect("validated interface");
    let neighbors = neighbors(interface).await;
    for (address, mac) in neighbors {
        if !(first..=last).contains(&u32::from(address)) {
            continue;
        }
        let mut device = found.get(&address).cloned().unwrap_or(NetworkDevice {
            address,
            mac: None,
            open_ports: Vec::new(),
            responded: false,
        });
        device.mac = Some(mac);
        write_msg(send, &event(ScanEvent::Device(device))).await?;
    }
    if (first..=last).contains(&u32::from(interface.address)) {
        let mut local = found
            .get(&interface.address)
            .cloned()
            .unwrap_or(NetworkDevice {
                address: interface.address,
                mac: None,
                open_ports: Vec::new(),
                responded: true,
            });
        local.mac = Some(interface.mac.clone());
        local.responded = true; // This adapter belongs to the running host.
        write_msg(send, &event(ScanEvent::Device(local))).await?;
    }
    write_msg(send, &event(ScanEvent::Finished)).await?;
    send.finish().ok();
    Ok(())
}

async fn tcp_probe(address: Ipv4Addr, source: Ipv4Addr, port: u16) -> (u16, bool, bool) {
    let permits = PROBES.get_or_init(|| Arc::new(Semaphore::new(96)));
    let _permit = permits.acquire().await.expect("probe semaphore");
    let result = async {
        let socket = TcpSocket::new_v4()?;
        socket.bind(SocketAddr::from((source, 0)))?;
        socket.connect(SocketAddr::from((address, port))).await
    };
    match tokio::time::timeout(Duration::from_millis(400), result).await {
        Ok(Ok(_)) => (port, true, true),
        Ok(Err(e)) => (
            port,
            false,
            e.kind() == std::io::ErrorKind::ConnectionRefused,
        ),
        Err(_) => (port, false, false),
    }
}

async fn probe(address: Ipv4Addr, source: Ipv4Addr, ports: Arc<Vec<u16>>) -> NetworkDevice {
    let mut jobs = JoinSet::new();
    let mut pending = ports.iter();
    let mut device = NetworkDevice {
        address,
        mac: None,
        open_ports: Vec::new(),
        responded: false,
    };
    loop {
        // Six port tasks per address, sixteen addresses at once. Keep task
        // memory bounded too, rather than spawning every requested port.
        while jobs.len() < 6 {
            let Some(&port) = pending.next() else { break };
            jobs.spawn(tcp_probe(address, source, port));
        }
        let Some(result) = jobs.join_next().await else {
            break;
        };
        let Ok((port, open, responded)) = result else {
            continue;
        };
        if open {
            device.open_ports.push(port);
        }
        device.responded |= responded;
    }
    if !device.responded {
        device.responded = ping(address, source).await;
    }
    device.open_ports.sort_unstable();
    device
}

fn command(program: &str) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(program);
    cmd.kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    cmd
}

#[cfg(windows)]
async fn ping(address: Ipv4Addr, source: Ipv4Addr) -> bool {
    // Windows ping.exe can exit successfully for an ICMP unreachable reply.
    // Inspect the native reply status instead, without localized text or a
    // console process. The call is bounded even if its scan is cancelled.
    let permits = PROBES.get_or_init(|| Arc::new(Semaphore::new(96))).clone();
    let permit = permits.acquire_owned().await.expect("probe semaphore");
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        use windows_sys::Win32::{
            Foundation::INVALID_HANDLE_VALUE,
            NetworkManagement::IpHelper::{
                ICMP_ECHO_REPLY, IcmpCloseHandle, IcmpCreateFile, IcmpSendEcho2Ex,
            },
        };
        // SAFETY: the handle is created and closed here; the u64 reply buffer
        // is aligned and larger than ICMP_ECHO_REPLY + payload + ICMP header.
        unsafe {
            let handle = IcmpCreateFile();
            if handle == INVALID_HANDLE_VALUE {
                return false;
            }
            let payload = *b"SLIP";
            let mut reply = [0u64; 128];
            let count = IcmpSendEcho2Ex(
                handle,
                std::ptr::null_mut(),
                None,
                std::ptr::null(),
                u32::from_ne_bytes(source.octets()),
                u32::from_ne_bytes(address.octets()),
                payload.as_ptr().cast(),
                payload.len() as u16,
                std::ptr::null(),
                reply.as_mut_ptr().cast(),
                std::mem::size_of_val(&reply) as u32,
                500,
            );
            let live = count > 0 && (*(reply.as_ptr() as *const ICMP_ECHO_REPLY)).Status == 0;
            IcmpCloseHandle(handle);
            live
        }
    })
    .await
    .unwrap_or(false)
}

#[cfg(not(windows))]
async fn ping(address: Ipv4Addr, source: Ipv4Addr) -> bool {
    let mut cmd = command("ping");
    #[cfg(target_os = "macos")]
    cmd.args([
        "-n",
        "-c",
        "1",
        "-W",
        "500",
        "-S",
        &source.to_string(),
        &address.to_string(),
    ]);
    #[cfg(not(any(windows, target_os = "macos")))]
    cmd.args([
        "-n",
        "-c",
        "1",
        "-W",
        "1",
        "-I",
        &source.to_string(),
        &address.to_string(),
    ]);
    // An absent/restricted ping command just means TCP-only discovery. Raw
    // sockets and external scanners are never prerequisites.
    matches!(tokio::time::timeout(Duration::from_millis(900), cmd.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status()).await, Ok(Ok(s)) if s.success())
}

async fn neighbors(interface: &NetworkInterface) -> BTreeMap<Ipv4Addr, String> {
    #[cfg(target_os = "linux")]
    {
        return tokio::fs::read_to_string("/proc/net/arp")
            .await
            .unwrap_or_default()
            .lines()
            .filter_map(|line| {
                let f: Vec<_> = line.split_whitespace().collect();
                if f.len() < 6 || f[5] != interface.name || f[2] == "0x0" {
                    return None;
                }
                Some((f[0].parse().ok()?, valid_mac(f[3])?))
            })
            .collect();
    }
    #[cfg(not(target_os = "linux"))]
    {
        let mut cmd = command("arp");
        #[cfg(windows)]
        cmd.args(["-a", "-N", &interface.address.to_string()]);
        #[cfg(not(windows))]
        cmd.arg("-an");
        let Ok(Ok(output)) = tokio::time::timeout(Duration::from_secs(2), cmd.output()).await
        else {
            return BTreeMap::new();
        };
        parse_neighbors(&String::from_utf8_lossy(&output.stdout), &interface.name)
    }
}

fn valid_mac(text: &str) -> Option<String> {
    let parts: Vec<_> = text.split([':', '-']).collect();
    if parts.len() != 6 {
        return None;
    }
    let bytes: Vec<u8> = parts
        .iter()
        .map(|p| u8::from_str_radix(p, 16))
        .collect::<Result<_, _>>()
        .ok()?;
    if bytes.iter().all(|&b| b == 0) || bytes[0] & 1 != 0 {
        return None;
    }
    Some(
        bytes
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(":"),
    )
}

#[cfg(any(not(target_os = "linux"), test))]
fn parse_neighbors(text: &str, interface: &str) -> BTreeMap<Ipv4Addr, String> {
    text.lines()
        .filter_map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            // BSD arp lists every interface. Windows is filtered with -N above.
            if let Some(index) = fields.iter().position(|&f| f == "on")
                && fields.get(index + 1).copied() != Some(interface)
            {
                return None;
            }
            let address = fields
                .iter()
                .find_map(|f| f.trim_matches(['(', ')']).parse().ok())?;
            let mac = fields.iter().find_map(|f| valid_mac(f))?;
            Some((address, mac))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn probe_distinguishes_open_and_closed_ports() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let open = listener.local_addr().unwrap().port();
        let closed_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let closed = closed_listener.local_addr().unwrap().port();
        drop(closed_listener);
        let device = probe(
            Ipv4Addr::LOCALHOST,
            Ipv4Addr::LOCALHOST,
            Arc::new(vec![open, closed]),
        )
        .await;
        assert!(device.responded);
        assert_eq!(device.open_ports, vec![open]);
    }
    #[test]
    fn windows_and_bsd_neighbor_tables() {
        let windows = "  192.168.1.1  00-11-22-aa-bb-cc dynamic\n 224.0.0.1 01-00-5e-00-00-01 static\n 192.168.1.2 00-00-00-00-00-00 invalid";
        assert_eq!(parse_neighbors(windows, "Ethernet").len(), 1);
        let bsd = "? (192.168.1.1) at 0:11:22:aa:bb:cc on en0 ifscope [ethernet]\n? (192.168.1.2) at 0:11:22:aa:bb:dd on en1 ifscope [ethernet]";
        assert_eq!(parse_neighbors(bsd, "en0").len(), 1);
    }
}
