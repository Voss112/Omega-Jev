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
        .pool_idle_timeout(Duration::from_secs(90))
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

/// Loopback, private, link-local, CGNAT, unspecified, broadcast, ULA (incl. IPv4-mapped IPv6).
pub fn is_private_ip(ip: IpAddr) -> bool {
    let v4 = |ip: Ipv4Addr| {
        ip.is_private()
            || ip.is_loopback()
            || ip.is_link_local()
            || ip.is_unspecified()
            || ip.is_broadcast()
            || ip.octets()[0] == 0
            || (ip.octets()[0] == 100 && (ip.octets()[1] & 0xc0) == 64)
    };
    match ip {
        IpAddr::V4(ip) => v4(ip),
        IpAddr::V6(ip) => {
            ip.to_ipv4_mapped().is_some_and(v4)
                || ip.is_loopback()
                || ip.is_unspecified()
                || (ip.segments()[0] & 0xfe00) == 0xfc00
                || (ip.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

pub fn is_private_url(url: &Url) -> bool {
    let Some(host) = url.host_str() else { return true };
    let host = host.trim_matches(['[', ']']).to_lowercase();
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
        for url in
            ["http://127.0.0.1/", "http://[::ffff:10.0.0.1]/", "http://100.64.1.1/", "http://router.lan/", "http://0.0.0.0/"]
        {
            assert!(is_private_url(&Url::parse(url).unwrap()), "{url}");
        }
        assert!(!is_private_url(&Url::parse("https://docs.rs/").unwrap()));
    }
}
