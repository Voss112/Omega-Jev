use std::{ffi::OsStr, path::PathBuf};

pub use tokio::process::{Child, Command};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Secrets never reach child processes.
const SECRET_VARS: &[&str] = &[
    "MOONSHOT_API_KEY",
    "TYPESAFE_API_KEY",
    "DEEPSEEK_API_KEY",
    "ANTHROPIC_API_KEY",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "GOOGLE_APPLICATION_CREDENTIALS",
];

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

/// A command line the fastest way it can run: a plain `program args…` starts directly (no PowerShell start-up,
/// ~0.3 s saved per command on Windows); anything using shell syntax, a cmdlet or an alias goes through the shell.
pub fn line(line: &str) -> Command {
    match direct(line) {
        Some((program, args)) => {
            let mut cmd = command(program);
            cmd.args(args);
            cmd
        }
        None => shell(line),
    }
}

/// Shell syntax a direct start would not honour (pipes, redirections, variables, globs, escapes…).
const SHELL_SYNTAX: &[char] =
    &['|', '&', ';', '<', '>', '$', '`', '(', ')', '{', '}', '*', '?', '%', '^', '!', '[', ']', '@', '#', ',', '~', '=', '\n', '\r'];

/// PowerShell aliases: same name, another program than the one on PATH (curl, sort, where…).
const POWERSHELL_NAMES: &[&str] = &[
    "cat", "cd", "chdir", "clear", "cls", "compare", "copy", "cp", "curl", "del", "diff", "dir", "echo", "erase", "foreach", "h",
    "history", "kill", "ls", "man", "md", "measure", "mkdir", "mount", "move", "mv", "popd", "ps", "pushd", "pwd", "r", "rd", "ren",
    "rm", "rmdir", "sc", "select", "set", "sleep", "sort", "start", "tee", "type", "wget", "where", "write",
];

/// `(program, args)` when `line` can start without a shell. Windows only: elsewhere `sh` starts in a millisecond.
fn direct(line: &str) -> Option<(PathBuf, Vec<String>)> {
    if !cfg!(windows) || line.contains(SHELL_SYNTAX) {
        return None;
    }
    let words = split(line)?;
    let (name, args) = words.split_first()?;
    let lower = name.to_lowercase();
    // `Verb-Noun` is a cmdlet; a bare alias would start a different program than PowerShell does.
    if POWERSHELL_NAMES.contains(&lower.as_str()) || (name.contains('-') && name.chars().next().is_some_and(char::is_uppercase)) {
        return None;
    }
    let program = resolve(name)?;
    // A batch file re-parses its arguments through cmd.exe: quoted or spaced ones stay with the shell.
    let batch = program.extension().is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"));
    if batch && args.iter().any(|a| a.is_empty() || a.contains(char::is_whitespace) || a.contains('"')) {
        return None;
    }
    Some((program, args.to_vec()))
}

/// Whitespace-separated words, `"…"` and `'…'` grouping; `None` on an unterminated quote.
fn split(line: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    for c in line.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => word.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                started = true;
            }
            None if c.is_whitespace() => {
                if started || !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
                started = false;
            }
            None => word.push(c),
        }
    }
    if quote.is_some() {
        return None;
    }
    if started || !word.is_empty() {
        words.push(word);
    }
    Some(words)
}

/// The executable PATH would run for `name` (`.exe`, `.com`, `.cmd`, `.bat`), as `CreateProcess` needs it.
fn resolve(name: &str) -> Option<PathBuf> {
    const EXTS: [&str; 4] = ["exe", "com", "cmd", "bat"];
    if name.is_empty() || name.contains(['/', '\\', ':']) {
        return None;
    }
    let has_ext = |p: &std::path::Path| p.extension().is_some_and(|e| EXTS.iter().any(|x| e.eq_ignore_ascii_case(x)));
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let exact = dir.join(name);
        if has_ext(&exact) && exact.is_file() {
            return Some(exact);
        }
        EXTS.iter().map(|e| dir.join(format!("{name}.{e}"))).find(|p| p.is_file())
    })
}

pub fn shell_name() -> &'static str {
    if cfg!(windows) { "PowerShell" } else { "sh" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_keep_their_quotes_together() {
        assert_eq!(split(r#"git commit -m "deux mots" 'x y' """#).unwrap(), ["git", "commit", "-m", "deux mots", "x y", ""]);
        assert!(split(r#"echo "open"#).is_none());
    }

    #[test]
    fn shell_syntax_aliases_and_cmdlets_keep_the_shell() {
        for line in ["cargo test | tee x", "echo $env:PATH", "ls", "Get-ChildItem", "curl https://x", "FOO=1 cargo", "rm -r x"] {
            assert!(direct(line).is_none(), "{line}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_plain_program_starts_without_powershell() {
        let (program, args) = direct("cmd /c exit 0").expect("cmd.exe est sur le PATH");
        assert!(program.to_string_lossy().to_lowercase().ends_with("cmd.exe"), "{}", program.display());
        assert_eq!(args, ["/c", "exit", "0"]);
        assert!(direct("certainement-absent-du-path --x").is_none(), "introuvable : repli sur le shell");
    }
}
