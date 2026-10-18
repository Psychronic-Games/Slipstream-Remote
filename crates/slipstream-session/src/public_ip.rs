//! The remote host's observed internet address, independent of hardware
//! inventory. Keep external lookups bounded and shared across viewers.
use anyhow::{Context, Result, bail, ensure};
use std::{
    net::IpAddr,
    sync::OnceLock,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

const ENDPOINTS: [&str; 2] = ["https://api.ipify.org", "https://api64.ipify.org"];
const SUCCESS_TTL: Duration = Duration::from_secs(60);
const FAILURE_TTL: Duration = Duration::from_secs(15);
const MANUAL_TTL: Duration = Duration::from_secs(5);

type Answer = Result<IpAddr, String>;
#[derive(Default)]
struct Cache {
    answer: Option<(Instant, Answer)>,
}
impl Cache {
    fn recent(&self, refresh: bool) -> Option<Answer> {
        let (at, result) = self.answer.as_ref()?;
        let ttl = if refresh {
            MANUAL_TTL
        } else if result.is_ok() {
            SUCCESS_TTL
        } else {
            FAILURE_TTL
        };
        (at.elapsed() < ttl).then(|| result.clone())
    }

    async fn lookup(
        &mut self,
        client: &reqwest::Client,
        endpoints: &[&str],
        refresh: bool,
    ) -> Answer {
        if let Some(answer) = self.recent(refresh) {
            return answer;
        }
        let answer = lookup(client,endpoints).await.map_err(|_| "Public IP unavailable; check the remote computer's internet connection or try Refresh".to_string());
        self.answer = Some((Instant::now(), answer.clone()));
        answer
    }
}

pub(crate) async fn get(refresh: bool) -> Answer {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    let mut cache = CACHE
        .get_or_init(|| Mutex::new(Cache::default()))
        .lock()
        .await;
    if let Some(answer) = cache.recent(refresh) {
        return answer;
    }
    let client = slipstream_core::directory::http_client()
        .map_err(|_| "Public IP lookup could not start".to_string())?;
    cache.lookup(&client, &ENDPOINTS, refresh).await
}

async fn lookup(client: &reqwest::Client, endpoints: &[&str]) -> Result<IpAddr> {
    for &endpoint in endpoints {
        if let Ok(address) = fetch(client, endpoint).await {
            return Ok(address);
        }
    }
    bail!("No public-IP provider answered")
}

async fn fetch(client: &reqwest::Client, endpoint: &str) -> Result<IpAddr> {
    let url = reqwest::Url::parse(endpoint)?;
    let mut response = client
        .get(url.clone())
        .timeout(Duration::from_secs(4))
        .send()
        .await?
        .error_for_status()?;
    ensure!(
        response.url().origin() == url.origin(),
        "Public-IP lookup redirected to another origin"
    );
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            body.len() + chunk.len() <= 64,
            "Public-IP response too large"
        );
        body.extend_from_slice(&chunk);
    }
    parse(&body)
}

fn parse(body: &[u8]) -> Result<IpAddr> {
    let ip: IpAddr = std::str::from_utf8(body)?
        .trim()
        .parse()
        .context("Invalid public-IP reply")?;
    if let IpAddr::V6(v6) = ip
        && let Some(mapped) = v6.to_ipv4_mapped()
    {
        return parse(mapped.to_string().as_bytes());
    }
    let public = match ip {
        IpAddr::V4(ip) => {
            !ip.is_private()
                && !ip.is_loopback()
                && !ip.is_link_local()
                && !ip.is_documentation()
                && !ip.is_multicast()
                && !ip.is_broadcast()
                && ip.octets()[0] != 0
                && ip.octets()[0] < 240
                && !(ip.octets()[0] == 100 && (64..128).contains(&ip.octets()[1]))
        }
        IpAddr::V6(ip) => {
            !ip.is_unspecified()
                && !ip.is_loopback()
                && !ip.is_multicast()
                && !ip.is_unique_local()
                && !ip.is_unicast_link_local()
        }
    };
    ensure!(public, "The lookup did not return a public address");
    Ok(ip)
}

impl crate::RemoteFs {
    pub async fn public_ip(&self, refresh: bool) -> Result<IpAddr> {
        let reply = tokio::time::timeout(
            Duration::from_secs(12),
            self.ask_reply(slipstream_core::proto::StreamOpen::PublicIp { refresh }),
        )
        .await
        .context("The remote public-IP lookup timed out")?
        .context("The remote public-IP lookup did not answer; the host may need an update")?
        .2;
        match reply {
            slipstream_core::proto::StreamReply::PublicIp(address) => Ok(address),
            slipstream_core::proto::StreamReply::Error(error) => bail!("{error}"),
            other => bail!("Unexpected public-IP reply: {other:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn public_addresses_only() {
        assert_eq!(
            parse(b" 93.184.216.34\n").unwrap().to_string(),
            "93.184.216.34"
        );
        assert_eq!(
            parse(b"2606:4700:4700::1111").unwrap().to_string(),
            "2606:4700:4700::1111"
        );
        for body in [
            "10.0.0.1",
            "127.0.0.1",
            "0.0.0.0",
            "100.64.0.1",
            "169.254.1.1",
            "255.255.255.255",
            "224.0.0.1",
            "::1",
            "::ffff:127.0.0.1",
            "fd00::1",
            "fe80::1",
            "<html>error</html>",
            "93.184.216.34 extra",
        ] {
            assert!(parse(body.as_bytes()).is_err(), "{body}");
        }
    }

    async fn endpoint(body: &'static str) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut request = [0; 2048];
                let _ = socket.read(&mut request).await;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            }
        });
        format!("http://{address}")
    }

    #[tokio::test]
    async fn fallback_cache_manual_refresh_and_failure_recovery() {
        slipstream_core::init_crypto();
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let invalid = endpoint("not an IP").await;
        let valid = endpoint("93.184.216.34\n").await;
        let mut cache = Cache::default();
        let first = cache
            .lookup(&client, &[&invalid, &valid], false)
            .await
            .unwrap();
        // The fixture servers are gone; automatic and immediate manual reads use cache.
        assert_eq!(cache.lookup(&client, &[], false).await.unwrap(), first);
        assert_eq!(cache.lookup(&client, &[], true).await.unwrap(), first);
        cache.answer.as_mut().unwrap().0 = Instant::now() - Duration::from_secs(6);
        let next = endpoint("2606:4700:4700::1111").await;
        assert_ne!(cache.lookup(&client, &[&next], true).await.unwrap(), first);
        cache.answer.as_mut().unwrap().0 = Instant::now() - Duration::from_secs(61);
        let large =
            endpoint("xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx").await;
        assert!(cache.lookup(&client, &[&large], false).await.is_err());
        let recovered = endpoint("93.184.216.34").await;
        assert!(
            cache.lookup(&client, &[&recovered], false).await.is_err(),
            "Back off after failure"
        );
        cache.answer.as_mut().unwrap().0 = Instant::now() - Duration::from_secs(16);
        assert_eq!(
            cache.lookup(&client, &[&recovered], false).await.unwrap(),
            first
        );
    }
}
