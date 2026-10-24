use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use futures_util::StreamExt;
use reqwest::{
    Client, Response, Url,
    dns::{Addrs, Name, Resolve, Resolving},
    redirect,
};

/// Generic browser UA: no client fingerprint, no telemetry.
pub const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:128.0) Gecko/20100101 Firefox/128.0";

/// One shared client. `proxy` accepts http(s)://, socks5:// or socks5h:// (Tor: socks5h://127.0.0.1:9050).
/// SSRF-hardened: DNS answers and redirect targets pointing to private networks are refused.
pub fn client(proxy: &str) -> Result<Client> {
    // TLS crypto provider (ring); idempotent, errors only if already installed.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut builder = Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(15))
        // A stalled stream (no byte for 5 min, not even a keep-alive) fails instead of hanging the turn forever.
        .read_timeout(Duration::from_secs(300))
        // Warm connections between turns: no TCP + TLS handshake (~100-300 ms) before each model call.
        .pool_idle_timeout(Duration::from_secs(300))
        .tcp_keepalive(Duration::from_secs(60))
        .http2_keep_alive_interval(Duration::from_secs(30))
        .http2_keep_alive_while_idle(true)
        .dns_resolver(PublicOnly)
        .redirect(redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 8 {
                attempt.error("too many redirects")
            } else if is_private_url(attempt.url()) {
                attempt.error("redirect to a private address blocked")
            } else {
                attempt.follow()
            }
        }));
    let proxy = proxy.trim();
    if !proxy.is_empty() {
        builder = builder.proxy(reqwest::Proxy::all(proxy).context("proxy invalide")?);
    }
    builder.build().context("client HTTP")
}

/// Loopback, private, link-local, CGNAT, benchmark, reserved, multicast, broadcast, ULA, site-local,
/// and IPv4 hidden inside IPv6 (mapped, compatible, 6to4, NAT64).
pub fn is_private_ip(ip: IpAddr) -> bool {
    fn v4(ip: Ipv4Addr) -> bool {
        let [a, b, c, _] = ip.octets();
        ip.is_private()
            || ip.is_loopback()
            || ip.is_link_local()
            || ip.is_unspecified()
            || ip.is_broadcast()
            || ip.is_multicast()
            || a == 0
            || a >= 240
            || (a == 100 && (b & 0xc0) == 64)
            || (a == 198 && (b & 0xfe) == 18)
            || (a == 192 && b == 0 && c == 0)
    }
    match ip {
        IpAddr::V4(ip) => v4(ip),
        IpAddr::V6(ip) => {
            let s = ip.segments();
            let embedded = |hi: u16, lo: u16| v4(Ipv4Addr::from((u32::from(hi) << 16) | u32::from(lo)));
            ip.to_ipv4_mapped().is_some_and(v4)
                || ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00
                || (s[0] & 0xffc0) == 0xfe80
                || (s[0] & 0xffc0) == 0xfec0
                || (s[0] == 0x2002 && embedded(s[1], s[2]))
                || (s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] && embedded(s[6], s[7]))
                || (s[..6] == [0; 6] && embedded(s[6], s[7]))
        }
    }
}

pub fn is_private_url(url: &Url) -> bool {
    let Some(host) = url.host_str() else { return true };
    // `localhost.` (trailing root dot) resolves like `localhost`.
    let host = host.trim_matches(['[', ']']).trim_end_matches('.').to_lowercase();
    match host.parse::<IpAddr>() {
        Ok(ip) => is_private_ip(ip),
        Err(_) => {
            host == "localhost" || [".localhost", ".local", ".internal", ".lan", ".home.arpa"].iter().any(|s| host.ends_with(s))
        }
    }
}

/// Resolver that drops private addresses (defeats DNS rebinding). Unused through a proxy (remote DNS).
struct PublicOnly;

impl Resolve for PublicOnly {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let addrs: Vec<SocketAddr> =
                tokio::net::lookup_host((host.as_str(), 0)).await?.filter(|a| !is_private_ip(a.ip())).collect();
            if addrs.is_empty() {
                return Err(format!("{host}: no public address").into());
            }
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

/// Opens (or refreshes) a pooled TLS connection to `url`; the answer is irrelevant and nothing secret is sent.
pub async fn warm(http: &Client, url: &str) {
    if url.starts_with("https://") {
        let _ = http.head(url).timeout(Duration::from_secs(5)).send().await;
    }
}

/// Reads at most `limit` bytes of the body (memory-safe on huge responses).
pub async fn read_capped(res: Response, limit: usize) -> Result<Vec<u8>> {
    if let Some(len) = res.content_length() {
        ensure!(len as usize <= limit * 4, "response too large ({len} bytes)");
    }
    let mut body = Vec::new();
    let mut stream = res.bytes_stream();
    while let Some(chunk) = stream.next().await {
        body.extend_from_slice(&chunk?);
        if body.len() >= limit {
            body.truncate(limit);
            break;
        }
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_targets_are_detected() {
        for url in [
            "http://127.0.0.1/",
            "http://[::ffff:10.0.0.1]/",
            "http://100.64.1.1/",
            "http://router.lan/",
            "http://0.0.0.0/",
            "http://localhost./",
            "http://224.0.0.1/",
            "http://198.18.0.1/",
            "http://[2002:7f00:1::]/",
            "http://[64:ff9b::a00:1]/",
            "http://[::7f00:1]/",
            "http://[fec0::1]/",
        ] {
            assert!(is_private_url(&Url::parse(url).unwrap()), "{url}");
        }
        for url in ["https://docs.rs/", "https://[2606:4700:4700::1111]/", "https://8.8.8.8/"] {
            assert!(!is_private_url(&Url::parse(url).unwrap()), "{url}");
        }
    }
}
