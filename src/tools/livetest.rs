//! Tests that run while the agent codes. One plugin per ecosystem: the ecosystem's own watcher when installed
//! (cargo watch, vitest, jest --watchAll, ptw, gotestsum --watch, dotnet watch test, gradle --continuous),
//! otherwise Omega's built-in watcher (workspace polling + a nudge on every agent edit) re-running the one-shot command.
//! Results reach the model as `[live-tests]` notes; it no longer runs the suite itself.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, LazyLock, Mutex, PoisonError},
    time::{Duration, Instant, UNIX_EPOCH},
};

use regex::Regex;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, BufReader},
    sync::Notify,
    task::JoinHandle,
};

use super::{compress, shell};
use crate::infra::process;

/// A native watcher: availability probe, long-running command, end-of-run and failure markers in its output.
pub struct Watch {
    pub probe: Probe,
    /// How to get the watcher when it is missing (shown in the tools panel).
    pub install: &'static str,
    pub command: &'static str,
    pub end: &'static str,
    pub failed: &'static str,
}

pub enum Probe {
    /// `program args` exits 0.
    Command(&'static str),
    /// The project ships the tool locally (`node_modules/.bin/<name>`).
    NodeBin(&'static str),
}

pub struct Plugin {
    pub name: &'static str,
    /// Root files that reveal the ecosystem; a leading `.` is an extension.
    pub markers: &'static [&'static str],
    /// `package.json` must mention this (vitest, jest…); empty = no condition.
    pub needs: &'static str,
    /// One-shot run, steps stop at the first failure.
    pub once: &'static [&'static str],
    pub watch: Option<Watch>,
}

pub const PLUGINS: &[Plugin] = &[
    Plugin {
        name: "Rust · cargo",
        markers: &["Cargo.toml"],
        needs: "",
        once: &["cargo test --quiet"],
        watch: Some(Watch {
            probe: Probe::Command("cargo watch --version"),
            install: "cargo install cargo-watch",
            command: "cargo watch -x 'test --quiet'",
            end: r"\[(Finished running\. Exit status: \d+|Command was successful|Command error, exit code \d+)\]",
            failed: r"Exit status: [1-9]|Command error",
        }),
    },
    Plugin {
        name: "JS/TS · vitest",
        markers: &["package.json"],
        needs: "vitest",
        once: &["npx --no-install vitest run"],
        watch: Some(Watch {
            probe: Probe::NodeBin("vitest"),
            install: "npm i -D vitest",
            command: "npx --no-install vitest watch",
            end: r"(Waiting|Watching) for file changes",
            failed: r"(Test Files|Tests)\s+\d+ failed|\bFAIL\b",
        }),
    },
    Plugin {
        name: "JS/TS · jest",
        markers: &["package.json"],
        needs: "jest",
        once: &["npx --no-install jest"],
        watch: Some(Watch {
            probe: Probe::NodeBin("jest"),
            install: "npm i -D jest",
            command: "npx --no-install jest --watchAll",
            end: r"Ran all test suites",
            failed: r"Tests:.*\d+ failed|\bFAIL\b",
        }),
    },
    Plugin { name: "JS/TS · npm test", markers: &["package.json"], needs: "\"test\"", once: &["npm test --silent"], watch: None },
    Plugin {
        name: "Python · pytest",
        markers: &["pyproject.toml", "pytest.ini", "setup.cfg", "setup.py", "requirements.txt", "tox.ini"],
        needs: "",
        once: &["python -m pytest -q"],
        watch: Some(Watch {
            probe: Probe::Command("ptw --version"),
            install: "pip install pytest-watch",
            command: "ptw -- -q",
            end: r"(?m)^.*\b(passed|failed|errors?|no tests ran)\b.* in [\d.]+s",
            failed: r"\b\d+ (failed|errors?)\b",
        }),
    },
    Plugin {
        name: "Go · go test",
        markers: &["go.mod"],
        needs: "",
        once: &["go test ./..."],
        watch: Some(Watch {
            probe: Probe::Command("gotestsum --version"),
            install: "go install gotest.tools/gotestsum@latest",
            command: "gotestsum --watch",
            end: r"DONE \d+ tests",
            failed: r"DONE \d+ tests.*\d+ failures?",
        }),
    },
    Plugin {
        name: ".NET · dotnet test",
        markers: &[".sln", ".csproj", ".fsproj"],
        needs: "",
        once: &["dotnet test --nologo"],
        watch: Some(Watch {
            probe: Probe::Command("dotnet --version"),
            install: "SDK .NET (dotnet watch inclus)",
            command: "dotnet watch test --nologo",
            end: r"(Passed!|Failed!)\s+-",
            failed: r"Failed!",
        }),
    },
    Plugin {
        name: "Java · gradle",
        markers: &["build.gradle", "build.gradle.kts"],
        needs: "",
        once: &["gradle test --quiet"],
        watch: Some(Watch {
            probe: Probe::Command("gradle --version"),
            install: "Gradle (gradle.org)",
            command: "gradle test --continuous",
            end: r"BUILD (SUCCESSFUL|FAILED)",
            failed: r"BUILD FAILED",
        }),
    },
    Plugin { name: "Java · maven", markers: &["pom.xml"], needs: "", once: &["mvn -q test"], watch: None },
    Plugin {
        name: "C/C++ · ctest",
        markers: &["CMakeLists.txt"],
        needs: "",
        once: &["cmake -S . -B build", "cmake --build build", "ctest --test-dir build --output-on-failure"],
        watch: None,
    },
    Plugin { name: "Ruby · rspec", markers: &["Gemfile"], needs: "", once: &["bundle exec rspec"], watch: None },
    Plugin { name: "PHP · phpunit", markers: &["composer.json"], needs: "", once: &["vendor/bin/phpunit"], watch: None },
    Plugin { name: "Deno · deno test", markers: &["deno.json", "deno.jsonc"], needs: "", once: &["deno test -q"], watch: None },
];

/// The plugin that fits the workspace root, first match wins (vitest before jest before plain `npm test`).
pub fn detect(root: &Path) -> Option<&'static Plugin> {
    let names: Vec<String> = std::fs::read_dir(root)
        .map(|d| d.flatten().take(500).map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    let package = std::fs::read_to_string(root.join("package.json")).unwrap_or_default();
    PLUGINS.iter().find(|p| {
        let marked = p.markers.iter().any(|m| names.iter().any(|n| if m.starts_with('.') { n.ends_with(m) } else { n == m }));
        marked && (p.needs.is_empty() || package.contains(p.needs))
    })
}

/// Shell commands that only re-run the test suite: answered with the live result instead of a second run.
static TEST_COMMAND: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^\s*(cargo (test|nextest run)|npm( run)? test|npx (--no-install )?(vitest( run)?|jest)|(python -m )?pytest|go test|dotnet test|\.?/?gradlew? test|mvn( -q)? test|ctest|bundle exec rspec|deno test)\b[^|;&]*$",
    )
    .expect("test command regex")
});

pub fn is_test_command(cmd: &str) -> bool {
    TEST_COMMAND.is_match(cmd)
}

/// One finished run.
#[derive(Clone, Debug)]
pub struct Outcome {
    pub ok: bool,
    pub summary: String,
    pub finished: Instant,
    pub seq: u64,
}

impl Outcome {
    /// Line handed to the model.
    pub fn note(&self) -> String {
        if self.ok { "[live-tests] ✓ suite verte".to_owned() } else { format!("[live-tests] ✗ échec\n{}", self.summary) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Native,
    Builtin,
}

/// What the tools panel shows.
#[derive(Clone, Debug)]
pub struct Status {
    pub plugin: &'static str,
    /// Native watcher missing: how to install it.
    pub hint: Option<&'static str>,
    pub mode: Mode,
    pub running: bool,
    pub last: Option<Outcome>,
    pub error: Option<String>,
}

#[derive(Default)]
struct State {
    running: bool,
    last: Option<Outcome>,
    seq: u64,
    buf: String,
    error: Option<String>,
    /// Native watcher's shell (its whole tree is killed on stop).
    pid: Option<u32>,
}

pub struct Runner {
    plugin: &'static Plugin,
    mode: Mode,
    state: Arc<Mutex<State>>,
    nudge: Arc<Notify>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl Runner {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn status(&self) -> Status {
        let s = self.lock();
        let hint = self.plugin.watch.as_ref().filter(|_| self.mode == Mode::Builtin).map(|w| w.install);
        Status { plugin: self.plugin.name, hint, mode: self.mode, running: s.running, last: s.last.clone(), error: s.error.clone() }
    }

    /// Sequence number of the latest finished run (0 = none yet).
    pub fn seq(&self) -> u64 {
        self.lock().seq
    }

    /// The latest run when it is newer than `seen`.
    pub fn newer(&self, seen: u64) -> Option<Outcome> {
        self.lock().last.clone().filter(|o| o.seq > seen)
    }

    /// The agent just changed files: the built-in watcher re-runs at once (a native one sees the change itself).
    pub fn touch(&self) {
        if self.mode == Mode::Builtin {
            self.nudge.notify_one();
        }
    }

    /// First run that finishes after `since`; `None` if the watcher stays idle 10 s or `timeout` passes.
    pub async fn wait_after(&self, since: Instant, timeout: Duration) -> Option<Outcome> {
        let start = Instant::now();
        loop {
            {
                let s = self.lock();
                if let Some(o) = s.last.as_ref().filter(|o| o.finished > since) {
                    return Some(o.clone());
                }
                if s.error.is_some() || (!s.running && start.elapsed() > Duration::from_secs(10)) {
                    return None;
                }
            }
            if start.elapsed() > timeout {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    /// The run in progress when there is one, else the last finished run.
    pub async fn latest(&self, timeout: Duration) -> Option<Outcome> {
        let (running, last) = {
            let s = self.lock();
            (s.running, s.last.clone())
        };
        if running { self.wait_after(Instant::now(), timeout).await.or(last) } else { last }
    }

    fn alive(&self) -> bool {
        self.task.lock().unwrap_or_else(PoisonError::into_inner).as_ref().is_some_and(|t| !t.is_finished())
    }

    fn stop(&self) {
        // Locks released first: `taskkill` blocks, and the output readers need the state meanwhile.
        let pid = self.lock().pid.take();
        if let Some(pid) = pid {
            shell::kill_tree(pid);
        }
        let task = self.task.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(task) = task {
            task.abort();
        }
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        self.stop();
    }
}

static RUNNERS: LazyLock<Mutex<HashMap<PathBuf, Arc<Runner>>>> = LazyLock::new(Default::default);

fn runners() -> std::sync::MutexGuard<'static, HashMap<PathBuf, Arc<Runner>>> {
    RUNNERS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The workspace's runner, started on first use (and restarted if its watcher died). `None`: no test ecosystem.
pub async fn ensure(root: &Path) -> Option<Arc<Runner>> {
    // A native watcher that died (crash, missing toolchain piece) is not retried: the built-in one takes over.
    let mut native_failed = false;
    if let Some(r) = runners().get(root).cloned() {
        if r.alive() {
            return Some(r);
        }
        native_failed = r.mode == Mode::Native;
    }
    runners().remove(root);
    let plugin = detect(root)?;
    let native = match &plugin.watch {
        Some(w) if !native_failed && available(root, &w.probe).await => Some(w),
        _ => None,
    };
    let runner = Arc::new(Runner {
        plugin,
        mode: if native.is_some() { Mode::Native } else { Mode::Builtin },
        state: Arc::default(),
        nudge: Arc::new(Notify::new()),
        task: Mutex::new(None),
    });
    let task = match native {
        Some(w) => tokio::spawn(native_loop(root.to_owned(), w, Arc::clone(&runner.state))),
        None => tokio::spawn(builtin_loop(root.to_owned(), plugin, Arc::clone(&runner.state), Arc::clone(&runner.nudge))),
    };
    *runner.task.lock().unwrap_or_else(PoisonError::into_inner) = Some(task);
    let mut map = runners();
    // Another turn may have started one meanwhile: keep the first, drop (and stop) ours.
    Some(Arc::clone(map.entry(root.to_owned()).or_insert(runner)))
}

/// The runner already watching `root`, without starting one.
pub fn current(root: &Path) -> Option<Arc<Runner>> {
    runners().get(root).cloned()
}

/// Stops the workspace's watcher: web content reached the turn, and tests would run code nobody approved.
pub fn stop(root: &Path) {
    let runner = runners().remove(root);
    if let Some(r) = runner {
        r.stop();
    }
}

/// Stops every watcher (option turned off, workspace changed, app closing): no orphan process survives.
pub fn stop_all() {
    let all: Vec<Arc<Runner>> = runners().drain().map(|(_, r)| r).collect();
    for r in all {
        r.stop();
    }
}

async fn available(root: &Path, probe: &Probe) -> bool {
    match probe {
        Probe::NodeBin(name) => {
            let bin = root.join("node_modules").join(".bin");
            bin.join(name).is_file() || bin.join(format!("{name}.cmd")).is_file()
        }
        Probe::Command(cmd) => shell::execute(root, cmd, 20).await.is_ok_and(|out| out.starts_with("exit 0")),
    }
}

/// Lines kept from a run for the model: failures and summary, never the whole log.
const SUMMARY_CHARS: usize = 2_500;
const BUF_CAP: usize = 400_000;

fn finish(state: &Mutex<State>, ok: bool, raw: &str, command: &str) {
    let mut s = state.lock().unwrap_or_else(PoisonError::into_inner);
    s.seq += 1;
    let seq = s.seq;
    s.last = Some(Outcome { ok, summary: compress::clip(&compress::command(command, raw), SUMMARY_CHARS), finished: Instant::now(), seq });
    s.running = false;
    s.buf.clear();
}

async fn native_loop(root: PathBuf, watch: &'static Watch, state: Arc<Mutex<State>>) {
    let (end, failed) = (Regex::new(watch.end).expect("end regex"), Regex::new(watch.failed).expect("failed regex"));
    let mut command = process::shell(watch.command);
    command.current_dir(&root).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    // Colour codes and spinners off: plain lines are easier to read and match.
    command.env("NO_COLOR", "1").env("FORCE_COLOR", "0").env("CARGO_TERM_COLOR", "never");
    #[cfg(unix)]
    command.process_group(0);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            state.lock().unwrap_or_else(PoisonError::into_inner).error = Some(format!("{}: {e}", watch.command));
            return;
        }
    };
    {
        let mut s = state.lock().unwrap_or_else(PoisonError::into_inner);
        s.pid = child.id();
        s.running = true;
    }
    let (Some(out), Some(err)) = (child.stdout.take(), child.stderr.take()) else { return };
    let read = |stream| pump(stream, Arc::clone(&state), end.clone(), failed.clone(), watch.command);
    tokio::join!(read(Box::new(out) as Box<dyn AsyncRead + Unpin + Send>), read(Box::new(err)));
    let status = child.wait().await;
    let mut s = state.lock().unwrap_or_else(PoisonError::into_inner);
    s.running = false;
    s.pid = None;
    s.error = Some(format!("{} s'est arrêté ({})", watch.command, status.map_or_else(|e| e.to_string(), |c| c.to_string())));
}

async fn pump(stream: Box<dyn AsyncRead + Unpin + Send>, state: Arc<Mutex<State>>, end: Regex, failed: Regex, command: &str) {
    let mut lines = BufReader::new(stream).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let line = compress::strip_ansi(&line);
        let done = {
            let mut s = state.lock().unwrap_or_else(PoisonError::into_inner);
            s.running = true;
            if s.buf.len() < BUF_CAP {
                s.buf.push_str(&line);
                s.buf.push('\n');
            }
            end.is_match(&line).then(|| s.buf.clone())
        };
        if let Some(raw) = done {
            finish(&state, !failed.is_match(&raw), &raw, command);
        }
    }
}

const POLL: Duration = Duration::from_secs(2);
const DEBOUNCE: Duration = Duration::from_millis(500);

async fn builtin_loop(root: PathBuf, plugin: &'static Plugin, state: Arc<Mutex<State>>, nudge: Arc<Notify>) {
    let mut seen;
    loop {
        state.lock().unwrap_or_else(PoisonError::into_inner).running = true;
        tokio::time::sleep(DEBOUNCE).await;
        seen = fingerprint_async(&root).await;
        // A change during the run restarts it: the result must describe the latest code.
        tokio::select! {
            (ok, raw) = run_once(&root, plugin) => finish(&state, ok, &raw, plugin.once.last().copied().unwrap_or_default()),
            () = nudge.notified() => continue,
        }
        loop {
            tokio::select! {
                () = nudge.notified() => break,
                () = tokio::time::sleep(POLL) => {
                    let now = fingerprint_async(&root).await;
                    if now != seen {
                        break;
                    }
                }
            }
        }
    }
}

/// `(passed, output)` of the plugin's steps, stopping at the first failure.
async fn run_once(root: &Path, plugin: &Plugin) -> (bool, String) {
    let mut out = String::new();
    for step in plugin.once {
        match shell::execute(root, step, 1800).await {
            Ok(text) => {
                let ok = text.starts_with("exit 0");
                out.push_str(&text);
                out.push('\n');
                if !ok {
                    return (false, out);
                }
            }
            Err(e) => return (false, format!("{out}{step}: {e:#}")),
        }
    }
    (true, out)
}

async fn fingerprint_async(root: &Path) -> u64 {
    let root = root.to_owned();
    tokio::task::spawn_blocking(move || fingerprint(&root)).await.unwrap_or(0)
}

/// Cheap change detector over the files git would track (sizes and modification times).
pub fn fingerprint(root: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let walk = ignore::WalkBuilder::new(root).hidden(true).git_ignore(true).build();
    for entry in walk.flatten().take(20_000) {
        let path = entry.path();
        if path.components().any(|c| matches!(c.as_os_str().to_str(), Some("target" | "node_modules" | ".omega" | ".git"))) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        path.hash(&mut h);
        meta.len().hash(&mut h);
        meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_nanos()).hash(&mut h);
    }
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let d = std::env::temp_dir().join(format!("omega-live-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        for (f, body) in files {
            std::fs::write(d.join(f), body).unwrap();
        }
        d
    }

    #[test]
    fn each_ecosystem_gets_its_plugin() {
        type Files<'a> = &'a [(&'a str, &'a str)];
        let cases: [(Files, Option<&str>); 6] = [
            (&[("Cargo.toml", "")], Some("Rust · cargo")),
            (&[("package.json", r#"{"devDependencies":{"vitest":"1"}}"#)], Some("JS/TS · vitest")),
            (&[("package.json", r#"{"devDependencies":{"jest":"29"}}"#)], Some("JS/TS · jest")),
            (&[("package.json", r#"{"scripts":{"test":"node t.js"}}"#)], Some("JS/TS · npm test")),
            (&[("App.csproj", "")], Some(".NET · dotnet test")),
            (&[("notes.md", "")], None),
        ];
        for (i, (files, want)) in cases.into_iter().enumerate() {
            let d = dir(&format!("detect{i}"), files);
            assert_eq!(detect(&d).map(|p| p.name), want);
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn markers_and_failures_are_recognised_per_watcher() {
        let rust = PLUGINS[0].watch.as_ref().unwrap();
        let (end, failed) = (Regex::new(rust.end).unwrap(), Regex::new(rust.failed).unwrap());
        assert!(end.is_match("[Finished running. Exit status: 101]") && failed.is_match("Exit status: 101"));
        assert!(end.is_match("[Finished running. Exit status: 0]") && !failed.is_match("[Finished running. Exit status: 0]"));
        let py = PLUGINS.iter().find(|p| p.name.starts_with("Python")).unwrap().watch.as_ref().unwrap();
        assert!(Regex::new(py.end).unwrap().is_match("1 failed, 3 passed in 0.12s"));
        assert!(Regex::new(py.failed).unwrap().is_match("1 failed, 3 passed in 0.12s"));
        assert!(!Regex::new(py.failed).unwrap().is_match("4 passed in 0.10s"));
        let go = PLUGINS.iter().find(|p| p.name.starts_with("Go")).unwrap().watch.as_ref().unwrap();
        assert!(Regex::new(go.failed).unwrap().is_match("DONE 12 tests, 2 failures in 1.3s"));
        assert!(!Regex::new(go.failed).unwrap().is_match("DONE 12 tests in 1.3s"));
    }

    #[test]
    fn only_plain_test_runs_are_intercepted() {
        for cmd in ["cargo test", "cargo test --quiet parser", "npm test", "npx vitest run", "python -m pytest -q", "go test ./..."] {
            assert!(is_test_command(cmd), "{cmd}");
        }
        for cmd in ["cargo build", "cargo test; rm x", "git status", "npm install", "cargo test | tee log"] {
            assert!(!is_test_command(cmd), "{cmd}");
        }
    }

    #[test]
    fn the_fingerprint_follows_file_changes() {
        let d = dir("fp", &[("a.rs", "fn a() {}")]);
        let before = fingerprint(&d);
        assert_eq!(before, fingerprint(&d), "stable sans changement");
        std::fs::write(d.join("a.rs"), "fn a() { b() }").unwrap();
        assert_ne!(before, fingerprint(&d));
        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn the_builtin_watcher_reports_a_failing_then_passing_suite() {
        // A `once` command that exists everywhere: the plugin's verdict follows its exit code.
        static FLIP: Plugin = Plugin {
            name: "test",
            markers: &[],
            needs: "",
            once: &[if cfg!(windows) { "if (Test-Path ok.txt) { exit 0 } else { exit 3 }" } else { "test -f ok.txt" }],
            watch: None,
        };
        let d = dir("builtin", &[("x.txt", "")]);
        let state: Arc<Mutex<State>> = Arc::default();
        let nudge = Arc::new(Notify::new());
        let task = tokio::spawn(builtin_loop(d.clone(), &FLIP, Arc::clone(&state), Arc::clone(&nudge)));
        let runner = Runner { plugin: &FLIP, mode: Mode::Builtin, state, nudge, task: Mutex::new(Some(task)) };
        let first = runner.wait_after(Instant::now() - Duration::from_secs(1), Duration::from_secs(60)).await.expect("1er run");
        assert!(!first.ok);
        let edit = Instant::now();
        std::fs::write(d.join("ok.txt"), "").unwrap();
        runner.touch();
        let second = runner.wait_after(edit, Duration::from_secs(60)).await.expect("2e run");
        assert!(second.ok && second.seq > first.seq, "{second:?}");
        drop(runner);
        let _ = std::fs::remove_dir_all(d);
    }

    /// A real Cargo project: red, then green after a fix. `cargo test -- --ignored live_cargo --nocapture`.
    #[tokio::test]
    #[ignore = "compile un projet cargo"]
    async fn live_cargo() {
        let d = dir("cargo", &[("Cargo.toml", "[package]\nname = \"livecheck\"\nversion = \"0.1.0\"\nedition = \"2021\"\n")]);
        std::fs::create_dir_all(d.join("src")).unwrap();
        let lib = |v: u8| format!("pub fn two() -> u8 {{ {v} }}\n#[test]\nfn t() {{ assert_eq!(two(), 2); }}\n");
        std::fs::write(d.join("src/lib.rs"), lib(3)).unwrap();
        let runner = ensure(&d).await.expect("projet Rust détecté");
        println!("mode {:?}", runner.status().mode);
        let red = runner.wait_after(Instant::now() - Duration::from_secs(1), Duration::from_secs(240)).await.expect("1er run");
        println!("{}", red.note());
        assert!(!red.ok);
        let edit = Instant::now();
        std::fs::write(d.join("src/lib.rs"), lib(2)).unwrap();
        runner.touch();
        let green = runner.wait_after(edit, Duration::from_secs(240)).await.expect("2e run");
        assert!(green.ok, "{}", green.summary);
        stop_all();
        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn a_native_watcher_run_is_read_from_its_output() {
        // Prints a cargo-watch style end marker, then keeps running like a real watcher.
        static FAKE: Watch = Watch {
            probe: Probe::Command("unused"),
            install: "",
            command: if cfg!(windows) {
                "Write-Output 'test x ... FAILED'; Write-Output '[Finished running. Exit status: 101]'; Start-Sleep 30"
            } else {
                "echo 'test x ... FAILED'; echo '[Finished running. Exit status: 101]'; sleep 30"
            },
            end: r"\[Finished running\. Exit status: \d+\]",
            failed: r"Exit status: [1-9]",
        };
        static PLUGIN: Plugin = Plugin { name: "fake", markers: &[], needs: "", once: &[], watch: None };
        let state: Arc<Mutex<State>> = Arc::default();
        let task = tokio::spawn(native_loop(std::env::temp_dir(), &FAKE, Arc::clone(&state)));
        let runner = Runner { plugin: &PLUGIN, mode: Mode::Native, state, nudge: Arc::new(Notify::new()), task: Mutex::new(Some(task)) };
        let outcome = runner.wait_after(Instant::now() - Duration::from_secs(1), Duration::from_secs(30)).await.expect("fin de run lue");
        assert!(!outcome.ok && outcome.summary.contains("FAILED"), "{outcome:?}");
        let pid = runner.lock().pid;
        assert!(pid.is_some(), "le watcher tourne encore");
        drop(runner);
    }
}
