//! VPN detection: local tunnel adapters + exit-IP ownership heuristics.

use std::time::Duration;

use reqwest::Client;
use serde_json::Value;

#[derive(Clone, Debug, Default)]
pub struct VpnInfo {
    pub active: bool,
    pub tunnel: Option<String>,
    pub ip: Option<String>,
    pub country: Option<String>,
    pub org: Option<String>,
}

/// Hosting / VPN operators commonly behind VPN exit nodes.
const ORG_HINTS: &[&str] = &[
    "vpn",
    "proton",
    "mullvad",
    "nord",
    "m247",
    "datacamp",
    "cdn77",
    "leaseweb",
    "hetzner",
    "ovh",
    "digitalocean",
    "vultr",
    "choopa",
    "linode",
    "akamai",
    "amazon",
    "google cloud",
    "microsoft",
    "oracle",
    "clouvider",
    "zenlayer",
    "xtom",
    "quadranet",
    "performive",
    "packethub",
    "tefincom",
    "hostroyale",
    "31173",
    "giganet",
    "creanova",
];

pub async fn detect(http: &Client, lookup: bool) -> VpnInfo {
    let tunnel = tokio::task::spawn_blocking(local_tunnel).await.ok().flatten();
    let mut info = VpnInfo { tunnel, ..VpnInfo::default() };
    if lookup && let Some(v) = public_ip(http).await {
        let text = |p: &str| v.pointer(p).and_then(Value::as_str).map(str::to_owned);
        info.ip = text("/ip");
        info.country = text("/country_code");
        info.org = text("/connection/org").or_else(|| text("/connection/isp"));
    }
    let org_vpn = info.org.as_deref().is_some_and(|o| {
        let o = o.to_lowercase();
        ORG_HINTS.iter().any(|h| o.contains(h))
    });
    info.active = info.tunnel.is_some() || org_vpn;
    info
}

async fn public_ip(http: &Client) -> Option<Value> {
    let res = http.get("https://ipwho.is/").timeout(Duration::from_secs(8)).send().await.ok()?;
    res.json().await.ok()
}

#[cfg(windows)]
fn local_tunnel() -> Option<String> {
    const HINTS: &[&str] = &[
        "wireguard",
        "wintun",
        "tap-windows",
        "tap0901",
        "openvpn",
        "protonvpn",
        "proton vpn",
        "mullvad",
        "nordlynx",
        "nordvpn",
        "expressvpn",
        "surfshark",
        "windscribe",
        "ivpn",
        "private internet access",
        "cyberghost",
        "vpn",
    ];
    let out = super::process::blocking("ipconfig").arg("/all").output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    adapters(&text).into_iter().find_map(|block| {
        let low = block.to_lowercase();
        (low.contains("ipv4") && HINTS.iter().any(|h| low.contains(h)))
            .then(|| block.lines().next().unwrap_or("VPN").trim().trim_end_matches(':').to_owned())
    })
}

/// Splits `ipconfig /all` output into one block per adapter (header lines start at column 0).
#[cfg(windows)]
fn adapters(text: &str) -> Vec<String> {
    let mut blocks: Vec<String> = Vec::new();
    for line in text.lines() {
        if !line.is_empty() && !line.starts_with(' ') {
            blocks.push(String::new());
        }
        if let Some(b) = blocks.last_mut() {
            b.push_str(line);
            b.push('\n');
        }
    }
    blocks
}

#[cfg(not(windows))]
fn local_tunnel() -> Option<String> {
    const PREFIXES: &[&str] = &["tun", "tap", "wg", "ppp", "proton", "nordlynx", "mullvad", "ipsec", "vpn"];
    std::fs::read_dir("/sys/class/net")
        .ok()?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .find(|name| PREFIXES.iter().any(|p| name.starts_with(p)))
}
