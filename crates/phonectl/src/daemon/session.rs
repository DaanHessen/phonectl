//! One authenticated connection to the phone, over any byte stream.
//!
//! Only one session is current at a time. A newly authenticated session
//! replaces the old one (the phone opens a Tailscale session while still on
//! Bluetooth and then drops Bluetooth), so there is never a period where both
//! deliver events.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use phone::link::{self, Hello, Message};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Instant, timeout};

use super::{Shared, Transport, now_millis};

/// Silence after which we ping. The phone answers from its socket thread, so
/// one ping costs it a single radio wakeup every 4 minutes at most, and none
/// while other traffic flows.
const PING_AFTER: Duration = Duration::from_secs(240);
const PONG_TIMEOUT: Duration = Duration::from_secs(30);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const CALL_TIMEOUT: Duration = Duration::from_secs(10);

type Pending = Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>;

pub struct Handle {
    pub id: u64,
    pub transport: Transport,
    tx: mpsc::Sender<Outgoing>,
    pending: std::sync::Arc<Pending>,
}

enum Outgoing {
    Message(Message),
    Close,
}

#[derive(Default)]
pub struct Registry {
    current: Mutex<Option<std::sync::Arc<Handle>>>,
    next_id: AtomicU64,
    next_call: AtomicU64,
}

impl Registry {
    pub fn current(&self) -> Option<std::sync::Arc<Handle>> {
        self.current.lock().unwrap().clone()
    }

    /// Queues a message for the phone. False when not connected.
    pub fn send(&self, message: Message) -> bool {
        match self.current() {
            Some(handle) => handle.tx.try_send(Outgoing::Message(message)).is_ok(),
            None => false,
        }
    }

    pub fn event(&self, topic: &str, data: Value) -> bool {
        self.send(Message::Event { topic: topic.into(), data })
    }

    /// Calls a method on the phone and waits for its result.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let handle = self.current().ok_or("phone not connected")?;
        let id = self.next_call.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = oneshot::channel();
        handle.pending.lock().unwrap().insert(id, tx);
        let message = Message::Call { id, method: method.into(), params };
        if handle.tx.send(Outgoing::Message(message)).await.is_err() {
            return Err("phone disconnected".into());
        }
        match timeout(CALL_TIMEOUT, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err("phone disconnected".into()),
            Err(_) => {
                handle.pending.lock().unwrap().remove(&id);
                Err("phone did not answer".into())
            }
        }
    }

    /// Closes the current session (for example before suspend).
    pub async fn close_current(&self) {
        if let Some(handle) = self.current() {
            let _ = handle.tx.send(Outgoing::Close).await;
        }
    }
}

/// Runs the server side of the handshake, then the session until it ends.
pub async fn run<S>(daemon: Shared, stream: S, transport: Transport, peer: String)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (read, mut write) = tokio::io::split(stream);
    let mut reader = BufReader::new(read);

    let hello = match timeout(HANDSHAKE_TIMEOUT, handshake(&daemon, &mut reader, &mut write)).await {
        Ok(Ok(hello)) => hello,
        Ok(Err(e)) => {
            tracing::warn!("{transport:?} handshake with {peer} failed: {e}");
            return;
        }
        Err(_) => {
            tracing::warn!("{transport:?} handshake with {peer} timed out");
            return;
        }
    };

    let id = daemon.sessions.next_id.fetch_add(1, Ordering::Relaxed) + 1;
    let (tx, mut rx) = mpsc::channel::<Outgoing>(256);
    let pending: std::sync::Arc<Pending> = Default::default();
    let handle = std::sync::Arc::new(Handle { id, transport, tx, pending: pending.clone() });
    let previous = daemon.sessions.current.lock().unwrap().replace(handle.clone());
    if let Some(previous) = previous {
        tracing::info!("replacing {:?} session", previous.transport);
        let _ = previous.tx.try_send(Outgoing::Close);
    }
    tracing::info!("phone connected over {transport:?}");
    {
        let mut state = daemon.state.lock().unwrap();
        state.transport = Some(transport);
        state.connected_since = Some(now_millis() / 1000);
        state.connecting_until = None;
        state.error = None;
        state.asleep = false;
        state.phone.device = hello.device.clone();
        if hello.tailscale_ip.is_some() {
            state.phone.tailscale_ip = hello.tailscale_ip.clone();
        }
        state.phone.app_version = hello.app_version.clone();
        state.phone.last_seen = Some(now_millis() / 1000);
    }
    daemon.save_phone();
    daemon.publish();
    daemon.event("connected", json!({"transport": transport}));
    // Features that sync state on (re)connect.
    daemon.clipboard.on_connected(&daemon);

    // Events are handled in order on their own task, so a feature that calls
    // back into the phone (icon fetch) never blocks this loop from reading
    // the answer.
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<(String, Value)>();
    let worker = {
        let daemon = daemon.clone();
        tokio::spawn(async move {
            while let Some((topic, data)) = event_rx.recv().await {
                dispatch_event(&daemon, &topic, data).await;
            }
        })
    };

    let mut line = String::new();
    let mut last_rx = Instant::now();
    let mut ping_sent: Option<Instant> = None;
    let mut ping_id = 0u64;
    let reason = loop {
        let deadline = match ping_sent {
            Some(sent) => sent + PONG_TIMEOUT,
            None => last_rx + PING_AFTER,
        };
        tokio::select! {
            read = read_line(&mut reader, &mut line) => {
                match read {
                    Ok(true) => {}
                    Ok(false) => break "phone closed the connection".to_string(),
                    Err(e) => break format!("read: {e}"),
                }
                last_rx = Instant::now();
                ping_sent = None;
                match serde_json::from_str::<Message>(&line) {
                    Ok(Message::Event { topic, data }) => { let _ = event_tx.send((topic, data)); }
                    Ok(message) => handle_message(&daemon, &pending, message),
                    Err(_) => tracing::warn!("malformed message from phone"),
                }
            }
            out = rx.recv() => {
                match out {
                    Some(Outgoing::Message(message)) => {
                        if let Err(e) = write_message(&mut write, &message).await {
                            break format!("write: {e}");
                        }
                    }
                    Some(Outgoing::Close) | None => break "closed by laptop".to_string(),
                }
            }
            _ = tokio::time::sleep_until(deadline) => {
                if ping_sent.is_some() {
                    break "phone stopped answering".to_string();
                }
                ping_id += 1;
                if let Err(e) = write_message(&mut write, &Message::Ping { id: ping_id }).await {
                    break format!("write: {e}");
                }
                ping_sent = Some(Instant::now());
            }
        }
    };

    // Flush a Sleeping (or anything else queued just before Close).
    while let Ok(Outgoing::Message(message)) = rx.try_recv() {
        let _ = write_message(&mut write, &message).await;
    }
    let _ = write.shutdown().await;
    for (_, waiter) in pending.lock().unwrap().drain() {
        let _ = waiter.send(Err("phone disconnected".into()));
    }
    // Let already-received events finish (a removal must not be lost).
    drop(event_tx);
    let _ = worker.await;

    let still_current = {
        let mut current = daemon.sessions.current.lock().unwrap();
        if current.as_ref().is_some_and(|h| h.id == id) {
            *current = None;
            true
        } else {
            false
        }
    };
    tracing::info!("{transport:?} session ended: {reason}");
    if still_current {
        {
            let mut state = daemon.state.lock().unwrap();
            state.transport = None;
            state.connected_since = None;
            state.phone.last_seen = Some(now_millis() / 1000);
            if !reason.starts_with("closed by laptop") {
                state.error = Some(reason.clone());
            }
        }
        daemon.save_phone();
        daemon.publish();
        daemon.event("disconnected", json!({"reason": reason}));
    }
}

async fn handshake<R, W>(daemon: &Shared, reader: &mut BufReader<R>, write: &mut W) -> anyhow::Result<Hello>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut line = String::new();
    if !read_line(reader, &mut line).await? {
        anyhow::bail!("closed before hello");
    }
    let hello: Hello = serde_json::from_str(&line)?;
    if hello.protocol != link::PROTOCOL_VERSION {
        write_json(write, &json!({"type": "error", "message": "unsupported protocol version"})).await?;
        anyhow::bail!("phone speaks protocol {}", hello.protocol);
    }
    let laptop_nonce = link::nonce();
    let proof = link::proof(&daemon.key, "laptop", &hello.nonce, &laptop_nonce);
    write_json(write, &json!({"type": "challenge", "nonce": laptop_nonce, "proof": proof})).await?;

    if !read_line(reader, &mut line).await? {
        anyhow::bail!("closed before auth (wrong pairing key on the phone?)");
    }
    let auth: Value = serde_json::from_str(&line)?;
    let expected = link::proof(&daemon.key, "phone", &hello.nonce, &laptop_nonce);
    let given = auth.get("proof").and_then(Value::as_str).unwrap_or_default();
    if auth.get("type").and_then(Value::as_str) != Some("auth") || !link::proof_matches(&expected, given) {
        write_json(write, &json!({"type": "error", "message": "authentication failed"})).await?;
        anyhow::bail!("phone failed authentication");
    }
    write_json(write, &json!({"type": "ready", "name": daemon.config.name})).await?;
    Ok(hello)
}

fn handle_message(daemon: &Shared, pending: &Pending, message: Message) {
    match message {
        Message::Event { .. } => unreachable!("events go to the worker"),
        Message::Result { id, data, error } => {
            if let Some(waiter) = pending.lock().unwrap().remove(&id) {
                let _ = waiter.send(match error {
                    Some(e) => Err(e),
                    None => Ok(data.unwrap_or(Value::Null)),
                });
            }
        }
        Message::Call { id, method, .. } => {
            // The phone does not call the laptop yet.
            daemon.sessions.send(Message::Result { id, data: None, error: Some(format!("unknown method {method}")) });
        }
        Message::Ping { id } => {
            daemon.sessions.send(Message::Pong { id: Some(id) });
        }
        Message::Pong { .. } | Message::Sleeping | Message::Unknown => {}
    }
}

async fn dispatch_event(daemon: &Shared, topic: &str, data: Value) {
    match topic {
        "status" => {
            let at = data.get("at").and_then(Value::as_i64).unwrap_or(0);
            let call = data.get("call").and_then(Value::as_str).map(str::to_owned);
            {
                let mut state = daemon.state.lock().unwrap();
                let previous_at = state.phone.status.as_ref().and_then(|s| s.get("at")).and_then(Value::as_i64).unwrap_or(0);
                // Status is a full snapshot; an older one (from a session that
                // was being replaced) must not overwrite a newer one.
                if at < previous_at {
                    return;
                }
                state.phone.status = Some(data);
                state.phone.last_seen = Some(now_millis() / 1000);
            }
            daemon.publish();
            if let Some(call) = call {
                daemon.media.call_state(&call).await;
            }
        }
        "call" => {
            if let Some(call) = data.get("state").and_then(Value::as_str) {
                daemon.media.call_state(call).await;
                if let Some(status) = daemon.state.lock().unwrap().phone.status.as_mut() {
                    status["call"] = Value::String(call.to_owned());
                }
                daemon.publish();
                daemon.event("call", json!({"state": call}));
            }
        }
        "clipboard" => match serde_json::from_value::<link::Clip>(data) {
            Ok(clip) => daemon.clipboard.remote(daemon, clip).await,
            Err(_) => tracing::warn!("malformed clipboard event"),
        },
        "notification" => match serde_json::from_value::<link::Notification>(data) {
            Ok(n) => {
                tracing::debug!("notification {} from {}", if n.update { "update" } else { "posted" }, n.package);
                daemon.notifier.show(daemon, n).await;
            }
            Err(e) => tracing::warn!("malformed notification event: {e}"),
        },
        "notification_removed" => {
            if let Some(key) = data.get("key").and_then(Value::as_str) {
                tracing::debug!("notification removed from {}", key.split('|').nth(1).unwrap_or("?"));
                daemon.notifier.remove(daemon, key).await;
            }
        }
        "notifications" => {
            let items: Vec<link::Notification> = data
                .get("items")
                .cloned()
                .and_then(|items| serde_json::from_value(items).ok())
                .unwrap_or_default();
            daemon.notifier.reconcile(daemon, items).await;
        }
        "update" => {
            let status = data.get("status").and_then(Value::as_str).unwrap_or("?").to_owned();
            tracing::info!("phone app update: {status}");
            daemon.event("update", json!({"status": status}));
        }
        other => tracing::debug!("ignoring event {other}"),
    }
}

/// Reads one line into `line` (without the newline). False on clean EOF.
async fn read_line<R: AsyncRead + Unpin>(reader: &mut BufReader<R>, line: &mut String) -> std::io::Result<bool> {
    line.clear();
    let mut bytes = Vec::new();
    let mut limited = reader.take(link::MAX_LINE as u64 + 1);
    let n = limited.read_until(b'\n', &mut bytes).await?;
    if n == 0 {
        return Ok(false);
    }
    if bytes.last() != Some(&b'\n') {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "line too long or truncated"));
    }
    bytes.pop();
    *line = String::from_utf8(bytes).map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "not UTF-8"))?;
    Ok(true)
}

async fn write_message<W: AsyncWrite + Unpin>(write: &mut W, message: &Message) -> std::io::Result<()> {
    let text = link::line(message).map_err(std::io::Error::other)?;
    timeout(Duration::from_secs(30), async {
        write.write_all(text.as_bytes()).await?;
        write.flush().await
    })
    .await
    .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "write stalled"))?
}

async fn write_json<W: AsyncWrite + Unpin>(write: &mut W, value: &Value) -> std::io::Result<()> {
    write.write_all(format!("{value}\n").as_bytes()).await?;
    write.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn read_line_handles_eof_and_limits() {
        let data: &[u8] = b"{\"a\":1}\nrest-without-newline";
        let mut reader = BufReader::new(data);
        let mut line = String::new();
        assert!(read_line(&mut reader, &mut line).await.unwrap());
        assert_eq!(line, "{\"a\":1}");
        assert!(read_line(&mut reader, &mut line).await.is_err());

        let empty: &[u8] = b"";
        let mut reader = BufReader::new(empty);
        assert!(!read_line(&mut reader, &mut line).await.unwrap());
    }
}
