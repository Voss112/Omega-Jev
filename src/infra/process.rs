use std::ffi::OsStr;

pub use tokio::process::Command;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Secrets never reach child processes.
const SECRET_VARS: &[&str] = &["MOONSHOT_API_KEY", "TYPESAFE_API_KEY", "DEEPSEEK_API_KEY"];

/// Program without console window flash (Windows GUI) and killed when dropped.
pub fn command(program: impl AsRef<OsStr>) -> Command {
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd.kill_on_drop(true);
    for var in SECRET_VARS {
        cmd.env_remove(var);
    }
    cmd
}

/// Blocking variant for quick probes (no console window either).
pub fn blocking(program: impl AsRef<OsStr>) -> std::process::Command {
    let mut cmd = std::process::Command::new(program);
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut cmd, CREATE_NO_WINDOW);
    for var in SECRET_VARS {
        cmd.env_remove(var);
    }
    cmd
}

/// `npm` is a `.cmd` shim on Windows.
pub fn npm() -> &'static str {
    if cfg!(windows) { "npm.cmd" } else { "npm" }
}

/// Native shell for the agent: PowerShell on Windows, sh on Unix.
pub fn shell(line: &str) -> Command {
    if cfg!(windows) {
        let mut cmd = command("powershell");
        cmd.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command"]);
        cmd.arg(format!("[Console]::OutputEncoding=[Text.Encoding]::UTF8; $ProgressPreference='SilentlyContinue'; {line}"));
        cmd
    } else {
        let mut cmd = command("sh");
        cmd.args(["-c", line]);
        cmd
    }
}

pub fn shell_name() -> &'static str {
    if cfg!(windows) { "PowerShell" } else { "sh" }
}
