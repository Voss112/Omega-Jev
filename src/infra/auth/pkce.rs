//! OAuth 2.0 PKCE (S256) helpers.

use ring::{
    digest,
    rand::{SecureRandom, SystemRandom},
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc7636_vector() {
        assert_eq!(challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        assert_eq!(encode("a b/c:d@e"), "a%20b%2Fc%3Ad%40e");
        let p = Pkce::new();
        assert_eq!(p.verifier.len(), 43);
        assert_eq!(p.challenge, challenge(&p.verifier));
        assert_eq!(p.state.len(), 64);
    }
}
