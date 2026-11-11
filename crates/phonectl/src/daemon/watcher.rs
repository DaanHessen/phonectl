//! Starts the phone's clipboard watcher (`ShellWatcher`) when it is not
//! running: after a phone reboot, an app update or a crash.
//!
//! The phone reports `status.shell_watcher` in every status snapshot. When it
//! says the watcher is down and the app may switch wireless debugging, we ask
//! it to open a short window (`adb_open`, it answers with adbd's port), connect
//! to that port over Tailscale with our own paired ADB key, and run exactly one
//! fixed command: [`START_COMMAND`]. Android then restarts phonectl with the
//! watcher loaded; the new process switches wireless debugging off again as
//! soon as the watcher runs (and after two minutes at the latest).
//!
//! Nothing the phone sends ends up in the command line: the phone only
//! supplies a port number, the address is the phone's known Tailscale IP.

use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use adb::HostKey;
use adb::discovery::{Discovered, Service};
use serde_json::{Value, json};

use super::{Shared, now_millis};

/// The only command the daemon ever runs on the phone by itself.
pub const START_COMMAND: &str = "am instrument com.daanh.phonectl/.ShellWatcher";

const OPEN_TIMEOUT: Duration = Duration::from_secs(45);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Waits between failed starts: a phone on mobile data or an untrusted Wi-Fi
/// cannot open wireless debugging, so do not ask it again every status.
const BACKOFF: [Duration; 5] = [
    Duration::from_secs(60),
    Duration::from_secs(5 * 60),
    Duration::from_secs(15 * 60),
    Duration::from_secs(60 * 60),
    Duration::from_secs(3 * 60 * 60),
];

#[derive(Default)]
pub struct Watcher {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    running: bool,
    failures: usize,
    next_try: Option<Instant>,
    last_start: Option<i64>,
    last_error: Option<String>,
}

/// What the phone said about its watcher.
#[derive(Debug, PartialEq, Eq)]
pub struct Report {
    pub running: bool,
    pub instrumented: bool,
    pub can_toggle: bool,
    pub error: Option<String>,
}

impl Report {
    pub fn from_status(status: &Value) -> Option<Report> {
        let w = status.get("shell_watcher")?;
        Some(Report {
            running: w.get("running").and_then(Value::as_bool).unwrap_or(false),
            instrumented: w.get("instrumented").and_then(Value::as_bool).unwrap_or(false),
            can_toggle: w.get("can_toggle_adb").and_then(Value::as_bool).unwrap_or(false),
            error: w.get("error").and_then(Value::as_str).map(str::to_owned),
        })
    }

    /// Worth a start: not running, not already being started by Android,
    /// and the app may open the debugging window itself.
    pub fn needs_start(&self) -> bool {
        !self.running && !self.instrumented && self.can_toggle
    }
}

impl Watcher {
    /// Called for every status snapshot; starts the watcher in the background
    /// when needed.
    pub fn on_status(&self, daemon: &Shared, status: &Value) {
        let Some(report) = Report::from_status(status) else { return };
        {
            let mut inner = self.inner.lock().unwrap();
            if report.running {
                inner.failures = 0;
                inner.next_try = None;
                inner.last_error = None;
                return;
            }
            if !report.needs_start() || inner.running {
                return;
            }
            if inner.next_try.is_some_and(|at| Instant::now() < at) {
                return;
            }
            inner.running = true;
        }
        let daemon = daemon.clone();
        tokio::spawn(async move {
            let result = start(&daemon).await;
            let watcher = &daemon.watcher;
            {
                let mut inner = watcher.inner.lock().unwrap();
                inner.running = false;
                inner.last_start = Some(now_millis() / 1000);
                // Even a start that worked waits before the next one, so a
                // watcher that dies right away cannot cause a restart loop.
                let wait = BACKOFF[inner.failures.min(BACKOFF.len() - 1)];
                inner.next_try = Some(Instant::now() + wait);
                inner.failures += 1;
                match &result {
                    Ok(()) => {
                        tracing::info!("clipboard watcher start sent to the phone");
                        inner.last_error = None;
                    }
                    Err(e) => {
                        tracing::warn!("clipboard watcher start failed: {e}");
                        inner.last_error = Some(e.clone());
                    }
                }
            }
            daemon.publish();
        });
    }

    /// A start that failed (no Wi-Fi, untrusted network) may work on a new
    /// connection, so forget its wait. A start that went through keeps its
    /// wait: `am instrument` itself drops the connection.
    pub fn on_connected(&self) {
        let mut inner = self.inner.lock().unwrap();
        if inner.last_error.is_some() {
            inner.next_try = None;
        }
    }

    pub fn status(&self) -> Value {
        let inner = self.inner.lock().unwrap();
        json!({
            "starting": inner.running,
            "last_start": inner.last_start,
            "last_error": inner.last_error,
        })
    }
}

async fn start(daemon: &Shared) -> Result<(), String> {
    let ip: IpAddr = daemon
        .state
        .lock()
        .unwrap()
        .phone
        .tailscale_ip
        .as_deref()
        .and_then(|ip| ip.parse().ok())
        .ok_or("phone's Tailscale address unknown")?;

    let opened = daemon.sessions.call_with_timeout("adb_open", json!({}), OPEN_TIMEOUT).await?;
    let port = parse_port(&opened)?;
    let result = run_start(SocketAddr::new(ip, port)).await;
    if result.is_err() {
        // The phone also closes it by itself after two minutes.
        let _ = daemon.sessions.call("adb_close", json!({})).await;
    }
    result
}

fn parse_port(opened: &Value) -> Result<u16, String> {
    opened
        .get("port")
        .and_then(Value::as_u64)
        .and_then(|p| u16::try_from(p).ok())
        .filter(|p| *p >= 1024)
        .ok_or_else(|| "phone sent no usable adb port".to_string())
}

async fn run_start(address: SocketAddr) -> Result<(), String> {
    let key = HostKey::load_or_generate(&phone::paths::adb_key()).map_err(|e| format!("adb key: {e}"))?;
    let discovered = Discovered { instance: String::new(), address, service: Service::Connect };
    let connected = tokio::time::timeout(CONNECT_TIMEOUT, phone::connect(discovered, &key, &phone::paths::key_comment()))
        .await
        .map_err(|_| "adb connect timed out".to_string())?
        .map_err(|e| match e {
            adb::Error::Unauthorized | adb::Error::Tls(_) => {
                "the phone does not know this laptop's adb key: run phonectl setup-clipboard".to_string()
            }
            other => format!("adb connect: {other}"),
        })?;
    let output = phone::shell(&connected.connection, &connected.banner, START_COMMAND)
        .await
        .map_err(|e| format!("adb shell: {e}"))?;
    let text = format!("{} {}", output.stdout_text(), output.stderr_text());
    if !output.ok() || text.contains("Error") || text.contains("Exception") {
        return Err(format!("am instrument failed: {}", text.trim().chars().take(200).collect::<String>()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_needs_start_only_when_down_and_allowed() {
        let status = json!({"shell_watcher": {"running": false, "instrumented": false, "can_toggle_adb": true}});
        assert!(Report::from_status(&status).unwrap().needs_start());
        let running = json!({"shell_watcher": {"running": true, "instrumented": true, "can_toggle_adb": true}});
        assert!(!Report::from_status(&running).unwrap().needs_start());
        let starting = json!({"shell_watcher": {"running": false, "instrumented": true, "can_toggle_adb": true}});
        assert!(!Report::from_status(&starting).unwrap().needs_start());
        let no_grant = json!({"shell_watcher": {"running": false, "instrumented": false, "can_toggle_adb": false}});
        assert!(!Report::from_status(&no_grant).unwrap().needs_start());
        assert!(Report::from_status(&json!({"clipboard": {}})).is_none());
    }

    #[test]
    fn port_must_be_a_plain_unprivileged_number() {
        assert_eq!(parse_port(&json!({"port": 40063})), Ok(40063));
        assert!(parse_port(&json!({"port": 22})).is_err());
        assert!(parse_port(&json!({"port": 70000})).is_err());
        assert!(parse_port(&json!({"port": "40063; reboot"})).is_err());
        assert!(parse_port(&json!({})).is_err());
    }

    #[test]
    fn start_command_is_fixed() {
        assert_eq!(START_COMMAND, "am instrument com.daanh.phonectl/.ShellWatcher");
        assert!(!START_COMMAND.contains(['$', ';', '&', '|', '`']));
    }
}
