//! Bounded, cancellable name discovery on the remote network. DNS uses the
//! host's own resolvers; NetBIOS and mDNS queries go only to discovered IPs.
//! DNS packet parsing (including compression) is provided by simple-dns.

use anyhow::{Result, bail};
use simple_dns::{Name, Packet, PacketFlag, QCLASS, QTYPE, Question, RCODE, TYPE, rdata::RData};
use slipstream_core::network::{DeviceName, NetworkInterface, ScanRequest, validate};
use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::{Arc, OnceLock},
    time::Duration,
};
use tokio::{net::UdpSocket, sync::Semaphore, task::JoinSet};

const WAIT: Duration = Duration::from_millis(850);
static LOOKUPS: OnceLock<Arc<Semaphore>> = OnceLock::new();

pub(crate) async fn lookup_batch(source: Ipv4Addr, addresses: Vec<Ipv4Addr>, interfaces: &[NetworkInterface]) -> Result<Vec<DeviceName>> {
    if addresses.len() > 16 {
        bail!("Choose at most 16 devices per name request");
    }
    for address in &addresses {
        validate(
            &ScanRequest {
                interface: source,
                cidr: format!("{address}/32"),
                ports: vec![80],
            },
            interfaces,
        )?;
    }
    let (hostname, dns) = tokio::task::spawn_blocking(slipstream_platform::machine::network_name_config).await?;
    let dns = Arc::new(
        dns.into_iter()
            .filter(|ip| !ip.is_unspecified())
            .take(3)
            .map(|ip| SocketAddr::new(ip, 53))
            .collect::<Vec<_>>(),
    );
    let mut jobs = JoinSet::new();
    for address in addresses {
        let (hostname, dns) = (hostname.clone(), dns.clone());
        jobs.spawn(async move {
            if address == source {
                return hostname.and_then(|name| clean_name(&name)).map(|name| DeviceName {
                    address,
                    name,
                    source: "Remote computer".into(),
                });
            }
            let permits = LOOKUPS.get_or_init(|| Arc::new(Semaphore::new(16)));
            let _permit = permits.acquire().await.ok()?;
            lookup(address, source, &dns).await
        });
    }
    let mut names = Vec::new();
    while let Some(result) = jobs.join_next().await {
        if let Some(name) = result? {
            names.push(name);
        }
    }
    Ok(names)
}

async fn lookup(address: Ipv4Addr, source: Ipv4Addr, dns: &[SocketAddr]) -> Option<DeviceName> {
    let mut jobs = JoinSet::new();
    for &server in dns {
        jobs.spawn(async move { dns_name(address, source, server, false).await.map(|name| (name, "Reverse DNS")) });
    }
    jobs.spawn(async move {
        dns_name(address, source, SocketAddr::from((address, 5353)), true)
            .await
            .map(|name| (name, "mDNS"))
    });
    jobs.spawn(async move { netbios_name(address, source).await.map(|name| (name, "NetBIOS")) });
    while let Some(Ok(result)) = jobs.join_next().await {
        if let Some((name, kind)) = result {
            return Some(DeviceName {
                address,
                name,
                source: kind.into(),
            });
        }
    }
    None
}

fn clean_name(name: &str) -> Option<String> {
    let name = name.trim().trim_end_matches('.');
    if name.is_empty() || name.len() > 253 || name.parse::<IpAddr>().is_ok() || name.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return None;
    }
    Some(name.to_string())
}

async fn exchange(source: Ipv4Addr, server: SocketAddr, query: &[u8]) -> Option<Vec<u8>> {
    tokio::time::timeout(WAIT, async {
        let bind = match server.ip() {
            IpAddr::V4(ip) if ip.is_loopback() => SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            IpAddr::V4(_) => SocketAddr::from((source, 0)),
            IpAddr::V6(ip) if ip.is_loopback() => SocketAddr::from((Ipv6Addr::LOCALHOST, 0)),
            IpAddr::V6(_) => SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0)),
        };
        let socket = UdpSocket::bind(bind).await.ok()?;
        socket.connect(server).await.ok()?; // Only accept this server's replies.
        socket.send(query).await.ok()?;
        let mut bytes = vec![0u8; 4096];
        let n = socket.recv(&mut bytes).await.ok()?;
        bytes.truncate(n);
        Some(bytes)
    })
    .await
    .ok()
    .flatten()
}

fn reverse_name(address: Ipv4Addr) -> String {
    let [a, b, c, d] = address.octets();
    format!("{d}.{c}.{b}.{a}.in-addr.arpa")
}

async fn dns_name(address: Ipv4Addr, source: Ipv4Addr, server: SocketAddr, mdns: bool) -> Option<String> {
    let id = rand::random::<u16>();
    let reverse = reverse_name(address);
    let mut query = Packet::new_query(id);
    if !mdns {
        query.set_flags(PacketFlag::RECURSION_DESIRED);
    }
    query.questions.push(Question::new(
        Name::new(&reverse).ok()?,
        QTYPE::TYPE(TYPE::PTR),
        QCLASS::CLASS(simple_dns::CLASS::IN),
        false,
    ));
    let bytes = exchange(source, server, &query.build_bytes_vec().ok()?).await?;
    parse_ptr(&bytes, id, &reverse, mdns)
}

fn parse_ptr(bytes: &[u8], id: u16, reverse: &str, mdns: bool) -> Option<String> {
    let response = Packet::parse(bytes).ok()?;
    if (response.id() != id && !(mdns && response.id() == 0))
        || !response.has_flags(PacketFlag::RESPONSE)
        || response.has_flags(PacketFlag::TRUNCATION)
        || response.rcode() != RCODE::NoError
    {
        return None;
    }
    let mut target = reverse.to_string();
    for _ in 0..8 {
        let mut alias = None;
        for answer in &response.answers {
            if !answer.name.to_string().trim_end_matches('.').eq_ignore_ascii_case(&target) {
                continue;
            }
            match &answer.rdata {
                RData::PTR(ptr) => return clean_name(&ptr.0.to_string()),
                RData::CNAME(cname) => alias = Some(cname.0.to_string()),
                _ => {}
            }
        }
        target = alias?.trim_end_matches('.').to_string();
    }
    None
}

/// RFC 1002 wildcard NBSTAT, with the 16-byte NetBIOS name encoded as A–P.
fn netbios_query(id: u16) -> Vec<u8> {
    let mut bytes = vec![0u8; 12];
    bytes[..2].copy_from_slice(&id.to_be_bytes());
    bytes[5] = 1; // one question
    bytes.push(32);
    let mut name = [0u8; 16];
    name[0] = b'*';
    for b in name {
        bytes.extend_from_slice(&[b'A' + (b >> 4), b'A' + (b & 15)]);
    }
    bytes.extend_from_slice(&[0, 0, 0x21, 0, 1]);
    bytes
}

async fn netbios_name(address: Ipv4Addr, source: Ipv4Addr) -> Option<String> {
    let id = rand::random::<u16>();
    let bytes = exchange(source, SocketAddr::from((address, 137)), &netbios_query(id)).await?;
    parse_netbios(&bytes, id)
}

fn skip_name(bytes: &[u8], position: &mut usize) -> Option<()> {
    // We only skip this field; compression targets are not dereferenced.
    for _ in 0..128 {
        let n = *bytes.get(*position)?;
        *position += 1;
        if n == 0 {
            return Some(());
        }
        if n & 0xc0 == 0xc0 {
            bytes.get(*position)?;
            *position += 1;
            return Some(());
        }
        if n > 63 {
            return None;
        }
        *position = position.checked_add(usize::from(n))?;
        if *position > bytes.len() {
            return None;
        }
    }
    None
}

fn parse_netbios(bytes: &[u8], id: u16) -> Option<String> {
    let word = |at: usize| -> Option<u16> { Some(u16::from_be_bytes(bytes.get(at..at + 2)?.try_into().ok()?)) };
    if word(0)? != id || word(2)? & 0x800f != 0x8000 {
        return None;
    }
    if word(4)? > 16 || word(6)? > 16 {
        return None;
    }
    let mut p = 12;
    for _ in 0..word(4)? {
        skip_name(bytes, &mut p)?;
        p = p.checked_add(4)?;
    }
    let mut fallback = None;
    for _ in 0..word(6)? {
        skip_name(bytes, &mut p)?;
        let kind = word(p)?;
        let len = usize::from(word(p + 8)?);
        p += 10;
        let data = bytes.get(p..p.checked_add(len)?)?;
        p += len;
        if kind != 0x21 {
            continue;
        }
        let count = usize::from(*data.first()?);
        if data.len() < 1 + count * 18 {
            return None;
        }
        for entry in data[1..1 + count * 18].as_chunks::<18>().0.iter() {
            let flags = u16::from_be_bytes([entry[16], entry[17]]);
            if flags & 0x8000 != 0 || ![0, 0x20].contains(&entry[15]) {
                continue;
            } // exclude workgroup/domain names
            let Some(name) = std::str::from_utf8(&entry[..15]).ok().and_then(clean_name) else {
                continue;
            };
            if entry[15] == 0 {
                return Some(name);
            }
            fallback = Some(name);
        }
    }
    fallback
}

#[cfg(test)]
mod tests {
    use super::*;
    use simple_dns::{CLASS, ResourceRecord, rdata::PTR};

    #[tokio::test]
    async fn reverse_dns_uses_the_selected_remote_resolver() {
        let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let target = server.local_addr().unwrap();
        let job = tokio::spawn(async move {
            let mut buf = [0u8; 512];
            let (n, peer) = server.recv_from(&mut buf).await.unwrap();
            let packet = Packet::parse(&buf[..n]).unwrap();
            assert_eq!(packet.questions[0].qname.to_string(), "9.1.168.192.in-addr.arpa");
            let mut reply = Packet::new_reply(packet.id());
            reply.answers.push(ResourceRecord::new(
                packet.questions[0].qname.clone(),
                CLASS::IN,
                60,
                RData::PTR(PTR(Name::new("OFFICE-PC.example.lan").unwrap())),
            ));
            server.send_to(&reply.build_bytes_vec().unwrap(), peer).await.unwrap();
        });
        assert_eq!(
            dns_name(Ipv4Addr::new(192, 168, 1, 9), Ipv4Addr::LOCALHOST, target, false).await.as_deref(),
            Some("OFFICE-PC.example.lan")
        );
        job.await.unwrap();
    }

    #[test]
    fn netbios_chooses_the_computer_not_the_workgroup() {
        let mut b = vec![0u8; 12];
        b[..2].copy_from_slice(&42u16.to_be_bytes());
        b[2] = 0x84;
        b[7] = 1;
        b.extend_from_slice(&[0xc0, 0x0c, 0, 0x21, 0, 1, 0, 0, 0, 0, 0, 37, 2]);
        let mut group = [b' '; 18];
        group[..9].copy_from_slice(b"WORKGROUP");
        group[15] = 0;
        group[16] = 0x84;
        group[17] = 0;
        let mut pc = [b' '; 18];
        pc[..9].copy_from_slice(b"OFFICE-PC");
        pc[15] = 0;
        pc[16] = 4;
        pc[17] = 0;
        b.extend_from_slice(&group);
        b.extend_from_slice(&pc);
        assert_eq!(parse_netbios(&b, 42).as_deref(), Some("OFFICE-PC"));
        assert!(parse_netbios(&b, 43).is_none());
        for n in 0..b.len() {
            assert!(parse_netbios(&b[..n], 42).is_none());
        }
        assert!(clean_name("bad\nname").is_none());
        assert!(clean_name("192.168.1.2").is_none());
    }
}
