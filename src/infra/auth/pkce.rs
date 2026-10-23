//! OAuth 2.0 PKCE (S256) helpers and the loopback redirect listener.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use ring::{
    digest,
    rand::{SecureRandom, SystemRandom},
};
use tokio::{
    io::AsyncReadExt,
    net::{TcpListener, TcpStream},
};

pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
    pub state: String,
}

impl Pkce {
    pub fn new() -> Self {
        let verifier = crate::infra::auth::b64_encode(&random(32));
        let challenge = challenge(&verifier);
        let state = random(32).iter().map(|b| format!("{b:02x}")).collect();
        Self { verifier, challenge, state }
    }
}

impl Default for Pkce {
    fn default() -> Self {
        Self::new()
    }
}

/// `BASE64URL(SHA256(verifier))`, RFC 7636.
pub fn challenge(verifier: &str) -> String {
    crate::infra::auth::b64_encode(digest::digest(&digest::SHA256, verifier.as_bytes()).as_ref())
}

/// RFC 3986 percent-encoding of the unreserved set.
pub fn encode(s: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(char::from(b)),
            _ => {
                out.push('%');
                out.push(char::from(HEX[usize::from(b >> 4)]));
                out.push(char::from(HEX[usize::from(b & 0x0f)]));
            }
        }
    }
    out
}

fn random(n: usize) -> Vec<u8> {
    let mut buf = vec![0u8; n];
    SystemRandom::new().fill(&mut buf).expect("CSPRNG");
    buf
}

/// Listens on `http://127.0.0.1:{port}/oauth2callback`, checks `state`, returns the code.
pub async fn await_code(port: u16, state: &str, timeout: Duration) -> Result<String> {
    let listener = TcpListener::bind(("127.0.0.1", port)).await.with_context(|| format!("écoute sur le port {port}"))?;
    tokio::time::timeout(timeout, serve(listener, state)).await.map_err(|_| anyhow::anyhow!("délai de connexion dépassé"))?
}

/// Reserves a free loopback port by binding and immediately releasing it.
pub async fn free_port() -> Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.context("port libre")?;
    Ok(listener.local_addr()?.port())
}

async fn serve(listener: TcpListener, state: &str) -> Result<String> {
    loop {
        let (mut socket, _) = listener.accept().await?;
        let mut buf = vec![0u8; 8192];
        let Ok(n) = socket.read(&mut buf).await else { continue };
        if n == 0 {
            continue;
        }
        let head = String::from_utf8_lossy(&buf[..n]);
        let Some(query) = head.split_whitespace().nth(1).and_then(|target| target.split_once('?')).map(|(_, q)| q) else {
            continue;
        };
        let params = parse_query(query);
        let state_ok = params.iter().any(|(k, v)| k == "state" && v == state);
        let body = if state_ok { "Connexion reussie. Revenez a Omega." } else { "Etat invalide. Recommencez." };
        respond(&mut socket, body).await;
        if !state_ok {
            bail!("state OAuth invalide");
        }
        if let Some((_, err)) = params.iter().find(|(k, _)| k == "error") {
            bail!("OAuth refusé : {err}");
        }
        let Some((_, code)) = params.iter().find(|(k, _)| k == "code") else {
            bail!("réponse OAuth sans code");
        };
        return Ok(code.clone());
    }
}

async fn respond(socket: &mut TcpStream, body: &str) {
    use tokio::io::AsyncWriteExt as _;
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.flush().await;
}

fn parse_query(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            Some((percent_decode(k), percent_decode(v)))
        })
        .collect()
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
                out.push(hex.unwrap_or(b'%'));
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt as _;

    #[test]
    fn rfc7636_vector() {
        assert_eq!(challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        assert_eq!(encode("a b/c:d@e"), "a%20b%2Fc%3Ad%40e");
        let p = Pkce::new();
        assert_eq!(p.verifier.len(), 43);
        assert_eq!(p.challenge, challenge(&p.verifier));
        assert_eq!(p.state.len(), 64);
    }

    async fn get(port: u16, query: &str) -> bool {
        for _ in 0..100 {
            if let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)).await {
                let req = format!("GET /oauth2callback?{query} HTTP/1.1\r\nHost: localhost\r\n\r\n");
                return stream.write_all(req.as_bytes()).await.is_ok();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        false
    }

    #[tokio::test]
    async fn loopback_captures_the_code_once() {
        let pkce = Pkce::new();
        let port = free_port().await.unwrap();
        let state = pkce.state.clone();
        let server = tokio::spawn(async move { await_code(port, &state, Duration::from_secs(5)).await });
        assert!(get(port, &format!("state={}&code=abc123", pkce.state)).await);
        assert_eq!(server.await.unwrap().unwrap(), "abc123");
    }

    #[tokio::test]
    async fn loopback_rejects_a_foreign_state() {
        let port = free_port().await.unwrap();
        let server = tokio::spawn(async move { await_code(port, "good", Duration::from_secs(5)).await });
        assert!(get(port, "state=bad&code=abc123").await);
        assert!(server.await.unwrap().is_err());
    }
}
