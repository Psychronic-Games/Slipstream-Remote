//! The computer's whole connection through the phone: a TUN device takes
//! every IPv4 packet, a userspace TCP/IP stack (`ipstack`) turns them back
//! into connections and flows, and each goes to the phone over the
//! [`Client`] link. Needs administrator (root) rights to create the device
//! and change routes and DNS, which are restored when the tunnel stops.

use std::{
    future::Future,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,
};

use anyhow::{Context, Result};
use ipstack::{IpStack, IpStackConfig, IpStackStream};
use tracing::{debug, info, warn};

use crate::Client;

/// The tunnel's own addresses (a private range unlikely to clash).
const TUN_IP: Ipv4Addr = Ipv4Addr::new(10, 233, 49, 2);
const TUN_GATEWAY: Ipv4Addr = Ipv4Addr::new(10, 233, 49, 1);
const TUN_NETMASK: Ipv4Addr = Ipv4Addr::new(255, 255, 255, 0);
const MTU: u16 = 1500;

#[derive(Debug, Clone)]
pub struct TunnelOptions {
    /// The DNS server the computer uses while tunnelled (queries go
    /// through the phone like any other traffic).
    pub dns: IpAddr,
    /// Windows: where `wintun.dll` is.
    pub wintun: Option<std::path::PathBuf>,
}

impl Default for TunnelOptions {
    fn default() -> Self {
        Self { dns: IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)), wintun: None }
    }
}

/// Sends the computer's traffic through `client` until `stop` completes or
/// the phone goes away, then puts its routes and DNS back. Returns why it
/// ended.
pub async fn run(client: Client, opts: TunnelOptions, stop: impl Future<Output = ()>) -> Result<String> {
    let phone = client.phone_addr();
    let mut config = tun::Configuration::default();
    config.address(TUN_IP).netmask(TUN_NETMASK).destination(TUN_GATEWAY).mtu(MTU).up();
    #[cfg(target_os = "linux")]
    config.tun_name("slipstream0");
    #[cfg(target_os = "windows")]
    let _wintun_file = match &opts.wintun {
        Some(_) => None,
        None => Some(wintun::extract().context("preparing the Wintun driver library")?),
    };
    #[cfg(target_os = "windows")]
    {
        config.tun_name("Subspace Relay");
        let wintun = opts.wintun.clone().or_else(|| _wintun_file.as_ref().map(|f| f.0.clone()));
        config.platform_config(move |p| {
            // A fixed GUID, so Windows treats each start as the same
            // network instead of asking about a new one every time.
            p.device_guid(0x5d1e_7a3c_4b2f_49e0_9c61_51b5_7e1a_c0de);
            if let Some(path) = &wintun {
                p.wintun_file(path);
            }
        });
    }
    #[cfg(target_os = "macos")]
    config.platform_config(|p| {
        p.packet_information(false);
    });
    let device = tun::create_as_async(&config).context("creating the network adapter (needs administrator rights)")?;
    let tun_name = tun::AbstractDevice::tun_name(&*device).context("network adapter name")?;
    info!(%tun_name, %phone, "Subspace Relay tunnel up");

    let args = tproxy_config::TproxyArgs::new()
        .tun_ip(TUN_IP.into())
        .tun_netmask(TUN_NETMASK.into())
        .tun_gateway(TUN_GATEWAY.into())
        .tun_dns(opts.dns)
        .tun_mtu(MTU)
        .tun_name(&tun_name)
        .proxy_addr(phone)
        .ipv6_default_route(false);
    let state = tproxy_config::tproxy_setup(&args).await.context("routing traffic into the tunnel")?;
    #[cfg(target_os = "linux")]
    route_linux_dns(&tun_name, opts.dns).await;

    let mut stack_config = IpStackConfig::default();
    stack_config.mtu_unchecked(MTU);
    stack_config.udp_timeout(Duration::from_secs(60));
    stack_config.packet_information(false);
    let mut stack = IpStack::new(stack_config, device);

    tokio::pin!(stop);
    let reason = loop {
        tokio::select! {
            _ = &mut stop => break "stopped".to_string(),
            why = client.closed() => break format!("the phone's relay went away: {why}"),
            accepted = stack.accept() => match accepted {
                Ok(IpStackStream::Tcp(tcp)) => {
                    let (client, target) = (client.clone(), destination(tcp.peer_addr(), opts.dns));
                    tokio::spawn(async move {
                        if let Err(e) = client.forward_tcp(target, tcp).await {
                            debug!(%target, "relayed TCP ended: {e:#}");
                        }
                    });
                }
                Ok(IpStackStream::Udp(udp)) => {
                    let (client, target) = (client.clone(), destination(udp.peer_addr(), opts.dns));
                    tokio::spawn(async move {
                        if let Err(e) = client.forward_udp(target, udp).await {
                            debug!(%target, "relayed UDP ended: {e:#}");
                        }
                    });
                }
                // ICMP (ping) and anything else can't go through an app's sockets.
                Ok(_) => {}
                Err(e) => break format!("the network adapter stopped: {e}"),
            },
        }
    };
    info!("Subspace Relay tunnel down: {reason}");
    drop(stack);
    if let Err(e) = tproxy_config::tproxy_remove(Some(state)).await {
        warn!("restoring routes and DNS: {e:#}");
    }
    client.close().await;
    Ok(reason)
}

/// `tproxy-config` gives every system the tunnel's gateway as its DNS
/// server (resolv.conf, the Windows adapter, the macOS service): lookups sent
/// there go to the chosen server through the phone.
fn destination(target: SocketAddr, dns: IpAddr) -> SocketAddr {
    if target.ip() == IpAddr::V4(TUN_GATEWAY) && target.port() == 53 {
        SocketAddr::new(dns, 53)
    } else {
        target
    }
}

/// With systemd-resolved, apps look names up through it rather than
/// `/etc/resolv.conf` (which `tproxy-config` sets): send every lookup to the
/// tunnel's DNS server. The settings go away with the adapter.
#[cfg(target_os = "linux")]
async fn route_linux_dns(tun: &str, dns: IpAddr) {
    // Tests in a network namespace would reach the host's resolved.
    if std::env::var_os("SLIPSTREAM_RELAY_NO_RESOLVED").is_some() {
        return;
    }
    let steps: [&[&str]; 3] = [&["dns", tun, &dns.to_string()], &["domain", tun, "~."], &["default-route", tun, "yes"]];
    for args in steps {
        match tokio::process::Command::new("resolvectl").args(args).output().await {
            Ok(out) if out.status.success() => {}
            Ok(out) => {
                debug!("resolvectl {args:?}: {}", String::from_utf8_lossy(&out.stderr).trim());
                return;
            }
            // Not a systemd-resolved system: /etc/resolv.conf is enough.
            Err(_) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookups_sent_to_the_tunnel_go_to_the_chosen_dns_server() {
        let dns = IpAddr::from([9, 9, 9, 9]);
        assert_eq!(destination(SocketAddr::from((TUN_GATEWAY, 53)), dns), SocketAddr::new(dns, 53));
        let web = SocketAddr::from(([93, 184, 215, 14], 443));
        assert_eq!(destination(web, dns), web);
        let other = SocketAddr::from((TUN_GATEWAY, 80));
        assert_eq!(destination(other, dns), other, "only DNS is redirected");
    }
}

/// Wintun's driver library travels inside the one Windows executable and is
/// written out for each tunnel (see `packaging/windows/wintun`).
#[cfg(target_os = "windows")]
mod wintun {
    use std::{io::Write, path::PathBuf};

    #[cfg(target_arch = "x86_64")]
    const DLL: &[u8] = include_bytes!("../../../packaging/windows/wintun/wintun.dll");
    #[cfg(not(target_arch = "x86_64"))]
    const DLL: &[u8] = &[];

    /// The written copy, removed when dropped (if Windows lets go of it).
    pub struct Extracted(pub PathBuf);

    impl Drop for Extracted {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// Writes the library under a name nobody could have prepared, in
    /// Windows' own temporary folder, which other users can't change files
    /// in. The tunnel runs as administrator and loads it from there.
    pub fn extract() -> std::io::Result<Extracted> {
        if DLL.is_empty() {
            return Err(std::io::Error::other("Subspace Relay's tunnel isn't available on this processor yet"));
        }
        let windows = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        let name = format!("slipstream-wintun-{:016x}{:016x}.dll", rand::random::<u64>(), rand::random::<u64>());
        let path = windows.join("Temp").join(name);
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&path)?;
        file.write_all(DLL)?;
        file.sync_all()?;
        Ok(Extracted(path))
    }
}
