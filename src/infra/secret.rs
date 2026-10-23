//! API keys at rest: DPAPI (bound to the Windows user account) on Windows, owner-only file on Unix.

use base64::Engine as _;

const PREFIX: &str = "dpapi:";

/// Encrypts for storage. Empty stays empty.
pub fn seal(plain: &str) -> String {
    if plain.is_empty() {
        return String::new();
    }
    platform::protect(plain.as_bytes()).map_or_else(|| plain.to_owned(), |blob| format!("{PREFIX}{}", b64().encode(blob)))
}

/// Decrypts a stored value; plain legacy values pass through unchanged.
pub fn open(stored: &str) -> String {
    let Some(encoded) = stored.strip_prefix(PREFIX) else { return stored.to_owned() };
    b64()
        .decode(encoded)
        .ok()
        .and_then(|blob| platform::unprotect(&blob))
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_default()
}

pub fn is_sealed(stored: &str) -> bool {
    stored.is_empty() || stored.starts_with(PREFIX)
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

#[cfg(windows)]
mod platform {
    use std::{ptr, slice};

    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData},
    };

    pub fn protect(data: &[u8]) -> Option<Vec<u8>> {
        call(data, |input, output| unsafe {
            CryptProtectData(input, ptr::null(), ptr::null(), ptr::null(), ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, output)
        })
    }

    pub fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
        call(data, |input, output| unsafe {
            CryptUnprotectData(input, ptr::null_mut(), ptr::null(), ptr::null(), ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, output)
        })
    }

    fn call(data: &[u8], f: impl FnOnce(*const CRYPT_INTEGER_BLOB, *mut CRYPT_INTEGER_BLOB) -> i32) -> Option<Vec<u8>> {
        let input = CRYPT_INTEGER_BLOB { cbData: u32::try_from(data.len()).ok()?, pbData: data.as_ptr().cast_mut() };
        let mut output = CRYPT_INTEGER_BLOB { cbData: 0, pbData: ptr::null_mut() };
        if f(&raw const input, &raw mut output) == 0 || output.pbData.is_null() {
            return None;
        }
        // SAFETY: on success the API returns a LocalAlloc'd buffer of `cbData` bytes that we own and free once.
        let bytes = unsafe { slice::from_raw_parts(output.pbData, output.cbData as usize) }.to_vec();
        unsafe { LocalFree(output.pbData.cast()) };
        Some(bytes)
    }
}

#[cfg(not(windows))]
mod platform {
    pub fn protect(_: &[u8]) -> Option<Vec<u8>> {
        None
    }

    pub fn unprotect(_: &[u8]) -> Option<Vec<u8>> {
        None
    }
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn dpapi_round_trip() {
        let sealed = super::seal("sk-test-123");
        assert!(sealed.starts_with(super::PREFIX) && !sealed.contains("sk-test"));
        assert_eq!(super::open(&sealed), "sk-test-123");
        assert_eq!(super::open("plain"), "plain");
    }
}
