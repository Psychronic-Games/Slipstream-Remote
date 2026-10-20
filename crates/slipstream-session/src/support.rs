//! Bounded support operations, separate from the screen/control stream.
use crate::RemoteFs;
use anyhow::{Result, bail, ensure};
use simple_dns::{CLASS, Name, Packet, PacketFlag, QCLASS, QTYPE, Question, TYPE, rdata::RData};
use slipstream_core::{
    config::ServerSettings,
    directory::DirectoryClient,
    network::NetworkInterface,
    proto::{InfoSection, StreamOpen, StreamReply},
};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub(crate) struct HostTools {
    pub station: ServerSettings,
    pub allow_wake: bool,
    pub status: Arc<dyn Fn(String) + Send + Sync>,
}

impl RemoteFs {
    pub async fn wake_on_lan(&self, interface: Ipv4Addr, mac: String) -> Result<()> {
        self.support_done(StreamOpen::WakeOnLan { interface, mac })
            .await
    }
    pub async fn support_status(&self, text: String) -> Result<()> {
        status_text(&text)?;
        self.support_done(StreamOpen::SupportStatus { text }).await
    }
    async fn support_done(&self, request: StreamOpen) -> Result<()> {
        let reply = tokio::time::timeout(Duration::from_secs(12), self.ask_reply(request))
            .await
            .map_err(|_| anyhow::anyhow!("The support request timed out"))?
            .map_err(|_| {
                anyhow::anyhow!("Support request unavailable; the host may need an update")
            })?
            .2;
        match reply {
            StreamReply::Done => Ok(()),
            StreamReply::Error(e) => bail!("{e}"),
            _ => bail!("Unexpected support reply"),
        }
    }
    pub async fn network_diagnostics(
        &self,
        interface: Ipv4Addr,
        name: String,
        comparison_dns: Option<IpAddr>,
    ) -> Result<Vec<InfoSection>> {
        let reply = tokio::time::timeout(
            Duration::from_secs(18),
            self.ask_reply(StreamOpen::NetworkDiagnostics {
                interface,
                name,
                comparison_dns,
            }),
        )
        .await
        .map_err(|_| anyhow::anyhow!("Network diagnostics timed out"))?
        .map_err(|_| {
            anyhow::anyhow!("Network diagnostics unavailable; the host may need an update")
        })?
        .2;
        match reply {
            StreamReply::NetworkDiagnostics(s) => Ok(s),
            StreamReply::Error(e) => bail!("{e}"),
            _ => bail!("Unexpected diagnostic reply"),
        }
    }
    pub async fn connect_through(
        &self,
        host: Ipv4Addr,
        port: u16,
    ) -> Result<crate::tunnel::Tunnel> {
        ensure!(
            port != 0 && !host.is_unspecified() && !host.is_multicast() && !host.is_broadcast(),
            "Choose a unicast device and a TCP port"
        );
        // Probe one connection before advertising a usable endpoint. The host
        // enforces its existing tunnel permission on this and every connection.
        let (_send, _recv, reply) = tokio::time::timeout(
            Duration::from_secs(12),
            self.ask_reply(StreamOpen::Tunnel {
                host: host.to_string(),
                port,
            }),
        )
        .await
        .map_err(|_| anyhow::anyhow!("The target did not answer through this workstation"))??;
        match reply {
            StreamReply::Ready => {}
            StreamReply::Error(e) => bail!("{e}"),
            _ => bail!("Unexpected tunnel reply"),
        }
        crate::tunnel::open(self.conn.clone(), 0, host.to_string(), port).await
    }
    pub async fn support_inventory(&self) -> Result<Vec<InfoSection>> {
        match tokio::time::timeout(Duration::from_secs(12), self.ask(StreamOpen::SystemInfo))
            .await??
            .2
        {
            StreamReply::SystemInfo(s) => Ok(s),
            _ => bail!("System inventory unavailable"),
        }
    }
}

pub fn status_text(text: &str) -> Result<String> {
    ensure!(
        text.len() <= 640 && text.chars().count() <= 160 && !text.chars().any(char::is_control),
        "Use a single status line of at most 160 characters"
    );
    Ok(text.trim().to_string())
}

pub fn magic_packet(mac: &str) -> Result<[u8; 102]> {
    let parts: Vec<_> = mac.split([':', '-']).collect();
    ensure!(
        parts.len() == 6
            && parts
                .iter()
                .all(|p| p.len() == 2 && p.bytes().all(|b| b.is_ascii_hexdigit())),
        "Enter a six-byte MAC address, e.g. 00:11:22:33:44:55"
    );
    let bytes: Vec<u8> = parts
        .into_iter()
        .map(|p| u8::from_str_radix(p, 16).unwrap())
        .collect();
    ensure!(
        bytes[0] & 1 == 0 && bytes.iter().any(|b| *b != 0),
        "Choose a nonzero unicast MAC address"
    );
    let mut packet = [0xff; 102];
    for chunk in packet[6..].as_chunks_mut::<6>().0.iter_mut() {
        chunk.copy_from_slice(&bytes);
    }
    Ok(packet)
}

pub(crate) async fn wake(
    source: Ipv4Addr,
    mac: &str,
    interfaces: &[NetworkInterface],
) -> Result<()> {
    let packet = magic_packet(mac)?;
    let adapter = interfaces
        .iter()
        .find(|i| i.address == source)
        .ok_or_else(|| anyhow::anyhow!("Refresh and choose a connected adapter"))?;
    ensure!(
        adapter.prefix <= 30,
        "This adapter has no broadcast subnet (/31 and /32 cannot send LAN wake packets)"
    );
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    {
        let mut last = LAST.lock().unwrap();
        ensure!(
            last.is_none_or(|at| at.elapsed() >= Duration::from_secs(2)),
            "Wait two seconds before sending another wake request"
        );
        *last = Some(Instant::now());
    }
    let broadcast =
        Ipv4Addr::from(u32::from(source) | !slipstream_core::network::mask(adapter.prefix));
    send_wake(source, SocketAddr::from((broadcast, 9)), &packet).await
}

async fn send_wake(source: Ipv4Addr, destination: SocketAddr, packet: &[u8; 102]) -> Result<()> {
    let socket = tokio::net::UdpSocket::bind((source, 0)).await?;
    socket.set_broadcast(true)?;
    for _ in 0..3 {
        socket.send_to(packet, destination).await?;
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name.len() <= 253
            && name
                .trim_end_matches('.')
                .split('.')
                .all(|label| !label.is_empty()
                    && label.len() <= 63
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-')),
        "Enter a DNS name such as example.com"
    );
    Ok(())
}

async fn dns(source: Ipv4Addr, server: SocketAddr, name: &str) -> Result<String> {
    let id = rand::random::<u16>();
    let mut query = Packet::new_query(id);
    query.set_flags(PacketFlag::RECURSION_DESIRED);
    query.questions.push(Question::new(
        Name::new(name)?,
        QTYPE::TYPE(TYPE::A),
        QCLASS::CLASS(CLASS::IN),
        false,
    ));
    let bytes = crate::network_names::exchange(source, server, &query.build_bytes_vec()?)
        .await
        .ok_or_else(|| anyhow::anyhow!("No UDP DNS answer (timeout or blocked)"))?;
    let response = Packet::parse(&bytes)?;
    ensure!(
        response.id() == id
            && response.has_flags(PacketFlag::RESPONSE)
            && response.questions.len() == 1
            && response.questions[0]
                .qname
                .to_string()
                .trim_end_matches('.')
                .eq_ignore_ascii_case(name.trim_end_matches('.'))
            && response.questions[0].qtype == QTYPE::TYPE(TYPE::A),
        "DNS answer did not match the query"
    );
    ensure!(
        !response.has_flags(PacketFlag::TRUNCATION),
        "UDP DNS answer was truncated; TCP fallback is not run"
    );
    ensure!(
        response.rcode() == simple_dns::RCODE::NoError,
        "DNS returned {:?}",
        response.rcode()
    );
    let mut addresses: Vec<_> = response
        .answers
        .iter()
        .filter_map(|r| match &r.rdata {
            RData::A(a) => Some(Ipv4Addr::from(a.address).to_string()),
            _ => None,
        })
        .collect();
    addresses.sort();
    addresses.dedup();
    Ok(if addresses.is_empty() {
        "No IPv4 answer".into()
    } else {
        addresses.join(", ")
    })
}

pub(crate) async fn diagnose(
    source: Ipv4Addr,
    name: String,
    comparison: Option<IpAddr>,
    station: &ServerSettings,
) -> Result<Vec<InfoSection>> {
    validate_name(&name)?;
    if let Some(ip) = comparison {
        ensure!(
            !ip.is_unspecified() && !ip.is_multicast() && ip != IpAddr::V4(Ipv4Addr::BROADCAST),
            "Choose a unicast comparison resolver"
        );
    }
    let interfaces =
        tokio::task::spawn_blocking(slipstream_platform::machine::network_interfaces).await?;
    let adapter = interfaces
        .iter()
        .find(|i| i.address == source)
        .ok_or_else(|| anyhow::anyhow!("Refresh and choose a connected adapter"))?;
    let mut rows = vec![
        (
            "Source adapter".into(),
            format!("{} · {source}", adapter.name),
        ),
        ("DNS query".into(), format!("{name} (IPv4/A)")),
    ];
    if let Some(gateway) = adapter.gateway {
        let at = Instant::now();
        let reachable = crate::network::ping(gateway, source).await;
        rows.push((
            format!("Gateway {gateway}"),
            if reachable {
                format!("ICMP response · {} ms", at.elapsed().as_millis())
            } else {
                "No ICMP response; it may be filtered".into()
            },
        ));
    } else {
        rows.push(("Gateway".into(), "Not reported for this adapter".into()));
    }
    let (_, resolvers) =
        tokio::task::spawn_blocking(slipstream_platform::machine::network_name_config).await?;
    let mut servers: Vec<_> = resolvers
        .into_iter()
        .filter(|ip| !ip.is_unspecified() && !ip.is_multicast())
        .take(3)
        .map(|ip| ("Configured DNS", ip))
        .collect();
    if let Some(ip) = comparison
        && !servers.iter().any(|(_, existing)| *existing == ip)
    {
        servers.push(("Comparison DNS", ip));
    }
    if servers.is_empty() {
        rows.push(("DNS".into(), "No configured resolver was reported".into()));
    }
    let mut jobs = tokio::task::JoinSet::new();
    for (label, ip) in servers {
        let name = name.clone();
        jobs.spawn(async move {
            let at = Instant::now();
            let answer = dns(source, SocketAddr::new(ip, 53), &name).await;
            (
                format!("{label} {ip}"),
                match answer {
                    Ok(answer) => format!("{answer} · {} ms", at.elapsed().as_millis()),
                    Err(e) => e.to_string(),
                },
            )
        });
    }
    let mut answers = Vec::new();
    while let Some(row) = jobs.join_next().await {
        answers.push(row?);
    }
    answers.sort();
    rows.extend(answers);
    let station_result = if let Some(url) = &station.url {
        let at = Instant::now();
        match DirectoryClient::new(url, station.key.clone()) {
            Ok(mut directory) => match tokio::time::timeout(Duration::from_secs(6), directory.hello()).await {
                Ok(Ok(_)) => format!("Verified directory hello · {} ms", at.elapsed().as_millis()),
                _ => "Directory hello failed or timed out; check station access, DNS, TLS and host clock".into(),
            },
            Err(_) => "Station address is invalid".into(),
        }
    } else {
        "No station configured".into()
    };
    rows.push(("Station API (host's route)".into(), station_result));
    rows.push(("Scope".into(), "Gateway/DNS probes use the selected adapter where supported; station HTTPS uses normal OS routing. This does not measure relay throughput or prove video-relay health.".into()));
    Ok(vec![InfoSection {
        title: "Network diagnostics".into(),
        rows,
    }])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wake_packet_and_status_validation() {
        let packet = magic_packet("00:11:22:33:44:55").unwrap();
        assert_eq!(&packet[..6], &[255; 6]);
        assert!(
            packet[6..]
                .as_chunks::<6>()
                .0
                .iter()
                .all(|c| *c == [0, 17, 34, 51, 68, 85])
        );
        for invalid in [
            "ff:ff:ff:ff:ff:ff",
            "00:00:00:00:00:00",
            "01:11:22:33:44:55",
            "00:11:22:33:44",
            "00:11:22:33:44:5g",
        ] {
            assert!(magic_packet(invalid).is_err());
        }
        assert_eq!(
            status_text("  Checking the printer  ").unwrap(),
            "Checking the printer"
        );
        assert!(status_text("hello\nworld").is_err());
        assert!(status_text(&"x".repeat(161)).is_err());
        for name in ["example.com", "printer.local", "example.com."] {
            assert!(validate_name(name).is_ok());
        }
        for name in [
            "",
            "a..b",
            "-bad.com",
            "example.com; rm",
            "http://example.com",
        ] {
            assert!(validate_name(name).is_err());
        }
    }
    #[tokio::test]
    async fn wake_sends_three_exact_packets_from_the_selected_source() {
        let receiver = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let packet = magic_packet("02:11:22:33:44:55").unwrap();
        send_wake(Ipv4Addr::LOCALHOST, receiver.local_addr().unwrap(), &packet)
            .await
            .unwrap();
        for _ in 0..3 {
            let mut bytes = [0; 256];
            let (len, peer) =
                tokio::time::timeout(Duration::from_secs(1), receiver.recv_from(&mut bytes))
                    .await
                    .unwrap()
                    .unwrap();
            assert_eq!(peer.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
            assert_eq!(&bytes[..len], &packet);
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(30), receiver.recv_from(&mut [0; 256]))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn dns_uses_selected_resolver_and_rejects_mismatched_replies() {
        use simple_dns::{ResourceRecord, rdata::A};
        for mismatch in [false, true] {
            let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let address = socket.local_addr().unwrap();
            let task = tokio::spawn(async move {
                let mut data = [0; 4096];
                let (n, peer) = socket.recv_from(&mut data).await.unwrap();
                let q = Packet::parse(&data[..n]).unwrap();
                let mut response = Packet::new_reply(q.id().wrapping_add(u16::from(mismatch)));
                response.questions = q.questions;
                response.answers.push(ResourceRecord::new(
                    Name::new("example.com").unwrap(),
                    CLASS::IN,
                    30,
                    RData::A(A {
                        address: u32::from(Ipv4Addr::new(192, 0, 2, 5)),
                    }),
                ));
                socket
                    .send_to(&response.build_bytes_vec().unwrap(), peer)
                    .await
                    .unwrap();
            });
            let result = dns(Ipv4Addr::LOCALHOST, address, "example.com").await;
            task.await.unwrap();
            if mismatch {
                assert!(result.is_err());
            } else {
                assert_eq!(result.unwrap(), "192.0.2.5");
            }
        }
    }
}
