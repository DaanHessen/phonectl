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
const PHONE_CALLS: &[&str] = &["ring", "ringer", "media_action", "test_notification", "diag"];

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
                    match daemon.sessions.call(phone_method, request["params"]["params"].clone()).await {
                        Ok(data) => json!({"ok": true, "data": data}),
                        Err(e) => json!({"ok": false, "error": e}),
                    }
                }
            }
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
