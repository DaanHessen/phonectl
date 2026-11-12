//! Local control socket: `$XDG_RUNTIME_DIR/phonectl/daemon.sock` (0700 dir).
//!
//! JSON lines. Requests: `{"method":"status"}`, `{"method":"subscribe"}`
//! (then a stream of `{"type":"status",…}` and `{"type":"event",…}` lines
//! until the client hangs up), `{"method":"poke"}`, `{"method":"clip"}`,
//! `{"method":"call","params":{"method":"ring","params":{"on":true}}}`.

use std::os::unix::fs::PermissionsExt;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

use super::Shared;

/// Phone methods local clients may invoke through the daemon.
const PHONE_CALLS: &[&str] = &[
    "ring", "ringer", "media_action", "test_notification", "diag", "type_text", "store_check", "dnd", "hotspot", "open_link",
];

/// Params a local client may pass, rebuilt from scratch so nothing else
/// reaches the phone. `dnd` and `hotspot` take exactly `{"on": bool}`.
fn clean_params(method: &str, params: &Value) -> Result<Value, &'static str> {
    match method {
        "dnd" | "hotspot" => match params.get("on").and_then(Value::as_bool) {
            Some(on) if params.as_object().is_some_and(|o| o.len() == 1) => Ok(json!({"on": on})),
            _ => Err("expects {\"on\": true|false}"),
        },
        "open_link" => match params.get("url").and_then(Value::as_str).and_then(super::notify::web_link) {
            Some(url) if params.as_object().is_some_and(|o| o.len() == 1) => Ok(json!({"url": url})),
            _ => Err("expects one http or https link"),
        },
        _ => Ok(params.clone()),
    }
}

/// AI agents run in the `ai.slice` cgroup (the airlock's Room B and its
/// sub-agents). Their calls are marked so the phone asks the owner first.
/// A caller whose cgroup cannot be read counts as an agent (fail closed).
fn caller_is_agent(cgroup: Option<&str>) -> bool {
    match cgroup {
        Some(text) => text.lines().any(|l| l.contains("/ai.slice/") || l.ends_with("/ai.slice")),
        None => true,
    }
}

fn peer_origin(stream: &UnixStream) -> Option<&'static str> {
    let cgroup = stream
        .peer_cred()
        .ok()
        .and_then(|c| c.pid())
        .and_then(|pid| std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok());
    caller_is_agent(cgroup.as_deref()).then_some("agent")
}

pub struct Guard(std::path::PathBuf);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

pub async fn serve(daemon: Shared) -> anyhow::Result<Guard> {
    let dir = phone::paths::runtime_dir();
    std::fs::create_dir_all(&dir)?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    let path = phone::paths::daemon_socket();
    if UnixStream::connect(&path).await.is_ok() {
        anyhow::bail!("another phonectl daemon is already running");
    }
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    tokio::spawn(client(daemon.clone(), stream));
                }
                Err(e) => tracing::warn!("ipc accept: {e}"),
            }
        }
    });
    Ok(Guard(path))
}

async fn client(daemon: Shared, stream: UnixStream) {
    let origin = peer_origin(&stream);
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let request: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
        let method = request.get("method").and_then(Value::as_str).unwrap_or("");
        let reply = match method {
            "status" => json!({"ok": true, "data": daemon.status()}),
            "poke" => {
                tokio::spawn(super::net::poke_burst(daemon.clone(), "user"));
                json!({"ok": true})
            }
            "clip" => match daemon.clipboard.push_now(&daemon).await {
                Ok(()) => json!({"ok": true}),
                Err(e) => json!({"ok": false, "error": e}),
            },
            "call" => {
                let phone_method = request["params"]["method"].as_str().unwrap_or("");
                if !PHONE_CALLS.contains(&phone_method) {
                    json!({"ok": false, "error": "not allowed"})
                } else {
                    match clean_params(phone_method, &request["params"]["params"]) {
                        Err(e) => json!({"ok": false, "error": e}),
                        Ok(params) => {
                            let limit = std::time::Duration::from_secs(if phone_method == "hotspot" { 30 } else { 10 });
                            match daemon.sessions.call_as(phone_method, params, origin, limit).await {
                                Ok(data) => json!({"ok": true, "data": data}),
                                Err(e) => json!({"ok": false, "error": e}),
                            }
                        }
                    }
                }
            }
            "update" if origin.is_some() => json!({"ok": false, "error": "agents cannot push app updates"}),
            "update" => match request["params"]["path"].as_str().map(std::fs::read) {
                Some(Ok(bytes)) => {
                    use base64::Engine;
                    let apk = base64::engine::general_purpose::STANDARD.encode(&bytes);
                    match daemon.sessions.call("install_update", json!({"apk": apk})).await {
                        Ok(data) => json!({"ok": true, "data": data}),
                        Err(e) => json!({"ok": false, "error": e}),
                    }
                }
                Some(Err(e)) => json!({"ok": false, "error": e.to_string()}),
                None => json!({"ok": false, "error": "path"}),
            },
            "subscribe" => {
                subscribe(&daemon, &mut write).await;
                return;
            }
            _ => json!({"ok": false, "error": "unknown method"}),
        };
        if write.write_all(format!("{reply}\n").as_bytes()).await.is_err() {
            return;
        }
    }
}

async fn subscribe(daemon: &Shared, write: &mut tokio::net::unix::OwnedWriteHalf) {
    let mut status = daemon.status_tx.subscribe();
    let mut events = daemon.events.subscribe();
    status.mark_changed();
    loop {
        let line = tokio::select! {
            changed = status.changed() => {
                if changed.is_err() { return; }
                let value = status.borrow_and_update().clone();
                json!({"type": "status", "data": value})
            }
            event = events.recv() => match event {
                Ok(event) => event,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => return,
            },
        };
        if write.write_all(format!("{line}\n").as_bytes()).await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PHONE_CALLS, caller_is_agent, clean_params};
    use serde_json::json;

    #[test]
    fn agents_are_recognised_by_cgroup_and_unknown_callers_count_as_agents() {
        assert!(caller_is_agent(Some("0::/user.slice/user-1000.slice/user@1000.service/ai.slice/run-p3953-i49312.scope\n")));
        assert!(caller_is_agent(None));
        assert!(!caller_is_agent(Some("0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-foot.scope\n")));
    }

    #[test]
    fn switch_params_are_a_single_bool() {
        assert_eq!(clean_params("hotspot", &json!({"on": true})).unwrap(), json!({"on": true}));
        assert_eq!(clean_params("dnd", &json!({"on": false})).unwrap(), json!({"on": false}));
        assert!(clean_params("hotspot", &json!({"on": "true"})).is_err());
        assert!(clean_params("hotspot", &json!({"on": true, "ssid": "x"})).is_err());
        assert!(clean_params("dnd", &json!(null)).is_err());
    }

    #[test]
    fn cli_cannot_reach_debugging_or_install_calls() {
        for privileged in ["adb_open", "adb_close", "install_update", "notification_action", "icon"] {
            assert!(!PHONE_CALLS.contains(&privileged), "{privileged} must not be callable over IPC");
        }
    }
}
