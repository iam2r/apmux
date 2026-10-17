//! Make a *running* CLI process pick up the config apmux just wrote.
//!
//! Claude Code, OpenCode, and Pi read their config when they start, so a
//! switch shows up in the next process. Codex does not: its TUI talks to a
//! long-lived local `app-server` daemon that loads `config.toml` and the
//! model catalog **once, at daemon start**. Starting a new Codex session
//! (or Ctrl+C'ing the old one) only re-attaches to that same daemon, so
//! `/model` keeps offering whatever the catalog held back then and a
//! provider switch looks like it did nothing.
//!
//! Restarting that daemon is what hands the new rows to already-open
//! sessions. apmux only ever *refreshes* a daemon that is already running:
//! `codex app-server daemon restart` would happily start one where none
//! existed, and starting daemons is explicitly outside apmux's scope.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::name;
use crate::paths::Paths;

/// Upper bound for `codex app-server daemon restart`. Measured ~0.7 s on a
/// warm daemon; the first run also installs the managed daemon package.
const RESTART_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// What happened while trying to reload a running CLI process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReloadOutcome {
    /// The app keeps no long-lived process holding the config apmux wrote.
    NotApplicable,
    /// No daemon is running: the next launch reads the new config anyway.
    NotRunning,
    /// The daemon was restarted, so already-open sessions see the new rows.
    Restarted { pid: Option<u32> },
    /// The restart attempt failed. The live config is written; sessions
    /// opened against the old daemon keep the old rows until it restarts.
    Failed(String),
}

/// The command apmux drives to refresh Codex's daemon.
fn daemon_args(action: &str) -> [&str; 3] {
    ["app-server", "daemon", action]
}

/// Codex: restart the local app-server daemon when one is running.
pub fn codex(paths: &Paths, bin: &Path) -> ReloadOutcome {
    reload_codex(&paths.codex_dir, bin)
}

/// Same, against an explicit Codex home (tests inject a temp home).
fn reload_codex(codex_dir: &Path, bin: &Path) -> ReloadOutcome {
    // The pid file marks a home whose daemon has run at least once. Checking
    // it first keeps apmux from exec'ing the CLI on machines that never used
    // the daemon — and keeps tests off the host.
    if !daemon_pid_file(codex_dir).is_file() {
        return ReloadOutcome::NotRunning;
    }
    if !daemon_running(bin) {
        return ReloadOutcome::NotRunning;
    }
    restart(bin)
}

fn daemon_pid_file(codex_dir: &Path) -> PathBuf {
    codex_dir
        .join(name::CODEX_DAEMON_DIR)
        .join(name::CODEX_DAEMON_PID_FILE)
}

/// `codex app-server daemon version` prints `{"status":"running",…}` on
/// stdout while a daemon answers; it fails when none does.
fn daemon_running(bin: &Path) -> bool {
    let Ok(out) = Command::new(bin)
        .args(daemon_args("version"))
        .stdin(Stdio::null())
        .output()
    else {
        return false;
    };
    out.status.success() && daemon_status(&out.stdout).as_deref() == Some("running")
}

fn restart(bin: &Path) -> ReloadOutcome {
    let mut child = match Command::new(bin)
        .args(daemon_args("restart"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => return ReloadOutcome::Failed(e.to_string()),
    };
    if !wait_within(&mut child, RESTART_TIMEOUT) {
        let _ = child.kill();
        let _ = child.wait();
        return ReloadOutcome::Failed(format!("timed out after {}s", RESTART_TIMEOUT.as_secs()));
    }
    match child.wait_with_output() {
        Ok(out) if out.status.success() => match daemon_status(&out.stdout).as_deref() {
            // "started" only happens if the daemon exited between the probe
            // and the restart; either way a daemon now holds the new config.
            Some("restarted" | "started") => ReloadOutcome::Restarted {
                pid: daemon_pid(&out.stdout),
            },
            _ => ReloadOutcome::Failed(format!(
                "unexpected reply from codex app-server daemon restart{}",
                stderr_tail(&out.stderr)
            )),
        },
        Ok(out) => ReloadOutcome::Failed(format!(
            "codex app-server daemon restart exited with {}{}",
            exit_label(&out.status),
            stderr_tail(&out.stderr)
        )),
        Err(e) => ReloadOutcome::Failed(e.to_string()),
    }
}

/// Poll a child until it exits or `timeout` elapses. `true` = it exited.
fn wait_within(child: &mut Child, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            // A reap error is surfaced by `wait_with_output`, not here.
            Err(_) => return true,
            Ok(None) => {
                if Instant::now() >= deadline {
                    return false;
                }
                std::thread::sleep(POLL_INTERVAL);
            }
        }
    }
}

fn exit_label(status: &std::process::ExitStatus) -> String {
    status
        .code()
        .map(|c| c.to_string())
        .unwrap_or_else(|| "signal".into())
}

/// Last JSON line on stdout carrying a `status`, e.g. `"restarted"`.
/// Progress events (stderr) and the final reply (stdout) are one JSON
/// object per line.
fn daemon_status(stdout: &[u8]) -> Option<String> {
    json_lines(stdout)
        .filter_map(|v| v.get("status")?.as_str().map(str::to_string))
        .next_back()
}

fn daemon_pid(stdout: &[u8]) -> Option<u32> {
    json_lines(stdout)
        .filter_map(|v| v.get("pid")?.as_u64())
        .filter_map(|pid| u32::try_from(pid).ok())
        .next_back()
}

fn json_lines(stdout: &[u8]) -> impl DoubleEndedIterator<Item = serde_json::Value> {
    String::from_utf8_lossy(stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line.trim()).ok())
        .collect::<Vec<_>>()
        .into_iter()
}

/// `": <last stderr line>"`, or `""` when there is nothing to add.
fn stderr_tail(stderr: &[u8]) -> String {
    String::from_utf8_lossy(stderr)
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .map(|line| format!(": {line}"))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A fake `codex`: answers the `app-server daemon version` probe with
    /// `probe` (a shell fragment that echoes and exits) and runs `restart`
    /// (a shell fragment that records the call) for any other subcommand.
    /// Unixes only: the fake is a `#!/bin/sh` script.
    #[cfg(unix)]
    fn fake_codex(probe: &str, restart: &str) -> (tempfile::TempDir, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fake-codex");
        let body = format!(
            "if [ \"$1 $2 $3\" = \"app-server daemon version\" ]; then\n\
             \t{probe}\n\
             fi\n\
             \t{restart}\n"
        );
        fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        (dir, path)
    }

    /// A Codex home; `with_pid` also plants the daemon pid file, which is
    /// what tells apmux a daemon has run there.
    #[cfg(unix)]
    fn daemon_home(with_pid: bool) -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        if with_pid {
            let dir = home.path().join(name::CODEX_DAEMON_DIR);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join(name::CODEX_DAEMON_PID_FILE), "{}").unwrap();
        }
        home
    }

    #[test]
    #[cfg(unix)]
    fn no_daemon_state_never_runs_the_cli() {
        let home = daemon_home(false);
        let (dir, bin) = fake_codex("echo probe >> \"$0.calls\"; exit 0", "exec true");
        let calls = PathBuf::from(format!("{}.calls", bin.display()));
        assert_eq!(reload_codex(home.path(), &bin), ReloadOutcome::NotRunning);
        assert!(!calls.exists(), "the CLI must not run without a pid file");
        drop(dir);
    }

    #[test]
    #[cfg(unix)]
    fn stale_pid_file_without_a_live_daemon_is_not_a_restart() {
        let home = daemon_home(true);
        let (dir, bin) = fake_codex("echo '{\"status\":\"notRunning\"}'\n exit 1", "exec true");
        assert_eq!(reload_codex(home.path(), &bin), ReloadOutcome::NotRunning);
        drop(dir);
    }

    #[test]
    #[cfg(unix)]
    fn running_daemon_is_restarted_and_reports_its_pid() {
        let home = daemon_home(true);
        let (dir, bin) = fake_codex(
            "echo '{\"status\":\"running\"}'\n exit 0",
            "echo restart >> \"$0\".calls\n echo '{\"status\":\"restarted\",\"pid\":4242}'",
        );
        let calls = PathBuf::from(format!("{}.calls", bin.display()));
        let outcome = reload_codex(home.path(), &bin);
        assert_eq!(outcome, ReloadOutcome::Restarted { pid: Some(4242) });
        assert_eq!(fs::read_to_string(&calls).unwrap(), "restart\n");
        drop(dir);
    }

    #[test]
    #[cfg(unix)]
    fn failed_restart_reports_the_cli_error() {
        let home = daemon_home(true);
        let (dir, bin) = fake_codex(
            "echo '{\"status\":\"running\"}'\n exit 0",
            "echo 'boom: socket busy' >&2\n exit 3",
        );
        match reload_codex(home.path(), &bin) {
            ReloadOutcome::Failed(msg) => {
                assert!(msg.contains("exited with 3"), "{msg}");
                assert!(msg.contains("boom: socket busy"), "{msg}");
            }
            other => panic!("expected failure, got {other:?}"),
        }
        drop(dir);
    }

    #[test]
    fn daemon_status_reads_the_last_json_line() {
        let stdout = b"{}\n{\"stage\":\"pid_write\",\"outcome\":\"succeeded\"}\n{\"status\":\"restarted\",\"pid\":7}\n";
        assert_eq!(daemon_status(stdout).as_deref(), Some("restarted"));
        assert_eq!(daemon_pid(stdout), Some(7));
        assert_eq!(daemon_status(b""), None);
        assert_eq!(daemon_status(b"not json"), None);
    }

    #[test]
    fn stderr_tail_is_empty_without_output() {
        assert_eq!(stderr_tail(b""), "");
        assert_eq!(stderr_tail(b"\n\n"), "");
        assert_eq!(stderr_tail(b"a\nlast line\n"), ": last line");
    }

    #[test]
    #[cfg(unix)]
    fn wait_within_gives_up_on_a_stuck_child() {
        let mut child = Command::new("sleep")
            .arg("30")
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        assert!(!wait_within(&mut child, Duration::from_millis(50)));
        let _ = child.kill();
        let _ = child.wait();
    }
}
