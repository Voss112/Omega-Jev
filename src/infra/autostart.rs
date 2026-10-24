//! Start with Windows: a value under the per-user `Run` key (no admin right, removed by turning it off).

use anyhow::Result;

const VALUE: &str = "Omega";

/// Command line the session runs at logon: the current executable, started minimized to the tray.
fn command() -> Result<String> {
    let exe = std::env::current_exe()?;
    Ok(format!("\"{}\" --background", exe.display()))
}

/// Brings the Run key in line with the setting; a no-op when it already matches.
pub fn apply(enabled: bool) -> Result<()> {
    if enabled { platform::set(VALUE, &command()?) } else { platform::remove(VALUE) }
}

#[cfg(windows)]
mod platform {
    use anyhow::{Result, bail};
    use windows_sys::Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS},
        System::Registry::{HKEY_CURRENT_USER, REG_SZ, RegDeleteKeyValueW, RegSetKeyValueW},
    };

    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    pub fn set(name: &str, command: &str) -> Result<()> {
        let data = wide(command);
        let bytes = u32::try_from(data.len() * 2)?;
        // SAFETY: NUL-terminated UTF-16 buffers that outlive the call; `bytes` is the exact data size.
        let code = unsafe {
            RegSetKeyValueW(HKEY_CURRENT_USER, wide(RUN).as_ptr(), wide(name).as_ptr(), REG_SZ, data.as_ptr().cast(), bytes)
        };
        if code != ERROR_SUCCESS {
            bail!("registre Run inaccessible (code {code})");
        }
        Ok(())
    }

    pub fn remove(name: &str) -> Result<()> {
        // SAFETY: NUL-terminated UTF-16 buffers that outlive the call.
        let code = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, wide(RUN).as_ptr(), wide(name).as_ptr()) };
        if code != ERROR_SUCCESS && code != ERROR_FILE_NOT_FOUND {
            bail!("registre Run inaccessible (code {code})");
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod platform {
    use anyhow::Result;

    pub fn set(_: &str, _: &str) -> Result<()> {
        Ok(())
    }

    pub fn remove(_: &str) -> Result<()> {
        Ok(())
    }
}
