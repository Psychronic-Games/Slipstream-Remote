//! Network discovery data shared by the host and the viewer.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;

pub const COMMON_PORTS: &str = "21,22,23,25,53,80,110,135,139,143,389,443,445,465,548,587,631,636,993,995,1433,1521,2049,3306,3389,5432,5900,5985,5986,8000,8080,8443,9100";
pub const MAX_HOSTS: u64 = 4096;
pub const MAX_PORTS: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkInterface {
    pub name: String,
    pub address: Ipv4Addr,
    pub prefix: u8,
    pub gateway: Option<Ipv4Addr>,
    pub mac: String,
}

impl NetworkInterface {
    pub fn cidr(&self) -> String {
        format!(
            "{}/{}",
            Ipv4Addr::from(u32::from(self.address) & mask(self.prefix)),
            self.prefix
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanRequest {
    pub interface: Ipv4Addr,
    pub cidr: String,
    pub ports: Vec<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkDevice {
    pub address: Ipv4Addr,
    pub mac: Option<String>,
    pub open_ports: Vec<u16>,
    /// A positive ICMP/TCP answer, or the scanning host's own address.
    /// A refused TCP connection also proves life.
    pub responded: bool,
}

/// Names are an optional auxiliary request, so existing protocol-6 hosts
/// and viewers can still scan together when only one side has been updated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceName {
    pub address: Ipv4Addr,
    pub name: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ScanEvent {
    Started { total: u32 },
    Device(NetworkDevice),
    Progress { completed: u32, total: u32 },
    Finished,
}

pub fn mask(prefix: u8) -> u32 {
    if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix.min(32))
    }
}

/// Inclusive usable range. /31 and /32 are point-to-point and single hosts.
pub fn subnet(cidr: &str) -> Result<(u32, u32)> {
    let Some((address, prefix)) = cidr.trim().split_once('/') else {
        bail!("Enter an IPv4 subnet, e.g. 192.168.1.0/24");
    };
    let address: Ipv4Addr = address
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid IPv4 address"))?;
    let prefix: u8 = prefix
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid subnet prefix"))?;
    if prefix > 32 {
        bail!("Subnet prefix must be 0–32");
    }
    let network = u32::from(address) & mask(prefix);
    let broadcast = network | !mask(prefix);
    Ok(if prefix < 31 {
        (network + 1, broadcast - 1)
    } else {
        (network, broadcast)
    })
}

pub fn parse_ports(text: &str) -> Result<Vec<u16>> {
    let mut ports = std::collections::BTreeSet::new();
    for item in text.split(',').map(str::trim) {
        let parse = |s: &str| -> Result<u16> {
            let p: u16 = s
                .trim()
                .parse()
                .map_err(|_| anyhow::anyhow!("Invalid TCP port: {s}"))?;
            if p == 0 {
                bail!("TCP ports start at 1");
            }
            Ok(p)
        };
        let (first, last) = match item.split_once('-') {
            Some((a, b)) => (parse(a)?, parse(b)?),
            None => {
                let p = parse(item)?;
                (p, p)
            }
        };
        if first > last {
            bail!("Port range is reversed: {item}");
        }
        if usize::from(last - first) + 1 > MAX_PORTS {
            bail!("Choose at most {MAX_PORTS} TCP ports per scan");
        }
        ports.extend(first..=last);
        if ports.len() > MAX_PORTS {
            bail!("Choose at most {MAX_PORTS} TCP ports per scan");
        }
    }
    if ports.is_empty() {
        bail!("Choose at least one TCP port");
    }
    Ok(ports.into_iter().collect())
}

/// Validate the source adapter and bound the requested range. The range may
/// be routed through a gateway (another subnet or VLAN).
pub fn validate(request: &ScanRequest, interfaces: &[NetworkInterface]) -> Result<(u32, u32)> {
    let interface = interfaces
        .iter()
        .find(|i| i.address == request.interface)
        .ok_or_else(|| {
            anyhow::anyhow!("That IPv4 adapter is no longer connected; refresh the adapters")
        })?;
    if interface.prefix > 32 {
        bail!("Invalid adapter subnet");
    }
    let (first, last) = subnet(&request.cidr)?;
    let count = u64::from(last) - u64::from(first) + 1;
    if count > MAX_HOSTS {
        bail!("This range has {count} addresses; narrow it to at most {MAX_HOSTS} (e.g. /24)");
    }
    if request.ports.is_empty() || request.ports.len() > MAX_PORTS || request.ports.contains(&0) {
        bail!("Choose 1–{MAX_PORTS} TCP ports, between 1 and 65535");
    }
    if count * request.ports.len() as u64 > 262_144 {
        bail!("Choose a smaller subnet or fewer ports (at most 262144 probes per scan)");
    }
    Ok((first, last))
}

pub fn service(port: u16) -> &'static str {
    match port {
        21 => "FTP",
        22 => "SSH",
        23 => "Telnet",
        25 | 465 | 587 => "SMTP",
        53 => "DNS",
        80 | 8000 | 8080 => "HTTP",
        110 | 995 => "POP3",
        135 => "RPC",
        139 => "NetBIOS",
        143 | 993 => "IMAP",
        389 | 636 => "LDAP",
        443 | 8443 => "HTTPS",
        445 => "SMB",
        548 => "AFP",
        631 => "IPP",
        1433 => "SQL Server",
        1521 => "Oracle",
        2049 => "NFS",
        3306 => "MySQL",
        3389 => "RDP",
        5432 => "PostgreSQL",
        5900 => "VNC",
        5985 | 5986 => "WinRM",
        9100 => "Printer",
        _ => "TCP",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn subnet_boundaries_and_limits() {
        assert_eq!(
            subnet("192.168.1.75/24").unwrap(),
            (
                u32::from(Ipv4Addr::new(192, 168, 1, 1)),
                u32::from(Ipv4Addr::new(192, 168, 1, 254))
            )
        );
        assert_eq!(subnet("10.0.0.2/31").unwrap(), (0x0a000002, 0x0a000003));
        assert_eq!(subnet("10.0.0.2/32").unwrap(), (0x0a000002, 0x0a000002));
        assert!(subnet("1.2.3.4/33").is_err());
        assert!(subnet("garbage").is_err());
        let interfaces = [NetworkInterface {
            name: "LAN".into(),
            address: Ipv4Addr::new(10, 0, 0, 2),
            prefix: 16,
            gateway: None,
            mac: String::new(),
        }];
        let mut r = ScanRequest {
            interface: interfaces[0].address,
            cidr: "10.0.0.0/24".into(),
            ports: vec![80],
        };
        assert!(validate(&r, &interfaces).is_ok());
        r.cidr = "10.1.0.0/24".into();
        assert!(validate(&r, &interfaces).is_ok(), "routed subnets and VLANs are allowed");
        r.cidr = "10.0.0.0/16".into();
        assert!(validate(&r, &interfaces).is_err());
        r.cidr = "172.16.2.1/32".into();
        r.interface = Ipv4Addr::new(10, 99, 0, 1);
        assert!(validate(&r, &interfaces).is_err(), "the source must still be a real remote adapter");
    }
    #[test]
    fn ports_are_bounded_and_deduplicated() {
        assert_eq!(parse_ports("443,80-82,80").unwrap(), vec![80, 81, 82, 443]);
        for s in ["", "0", "65536", "80-1", "1-65535", "abc"] {
            assert!(parse_ports(s).is_err(), "{s}");
        }
        assert!(parse_ports(COMMON_PORTS).is_ok());
    }
}
