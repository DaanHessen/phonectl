//! Laptop side of the clipboard sync, on wl-clipboard.
//!
//! `wl-paste --watch` (data-control protocol) runs a tiny command per
//! clipboard change that only prints a newline into our pipe: one line per
//! change, no polling. We then debounce (select-to-copy fires repeatedly while
//! a selection grows) and read the text once.
//!
//! Loop prevention mirrors the phone: the current clip is identified by its
//! hash; our own `wl-copy` of a phone clip produces a change whose hash is
//! already current and is dropped. Each clip carries its creation time so
//! after a reconnect the newer side wins. Clips marked secret by password
//! managers (`x-kde-passwordManagerHint: secret`) are never sent.

use std::process::Stdio;
use std::sync::Mutex;
use std::time::Duration;

use phone::link::{self, Clip};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use super::{Shared, now_millis};

#[derive(Default)]
struct Inner {
    hash: Option<String>,
    at: i64,
    /// A local clip the phone has not received yet.
    pending: Option<Clip>,
}

#[derive(Default)]
pub struct Clipboard {
    inner: Mutex<Inner>,
}

impl Clipboard {
    pub fn new() -> Self {
        Self::default()
    }

    /// The phone sent a clip.
    pub async fn remote(&self, daemon: &Shared, clip: Clip) {
        if clip.text.len() > link::MAX_CLIP || link::clip_hash(&clip.text) != clip.hash {
            tracing::warn!("ignoring malformed clipboard event");
            return;
        }
        {
            let mut inner = self.inner.lock().unwrap();
            if inner.hash.as_deref() == Some(&clip.hash) {
                return;
            }
            if clip.at < inner.at {
                // We changed ours after the phone made this one.
                return;
            }
            inner.hash = Some(clip.hash.clone());
            inner.at = clip.at;
            inner.pending = None;
        }
        if let Err(e) = write(&clip.text).await {
            tracing::warn!("wl-copy failed: {e}");
            return;
        }
        tracing::info!("clipboard: phone → laptop ({} bytes)", clip.text.len());
        daemon.event("clipboard", serde_json::json!({"from": "phone", "bytes": clip.text.len()}));
    }

    /// The laptop clipboard changed to `text`.
    fn local(&self, daemon: &Shared, text: String) {
        let clip = Clip::new(text, now_millis());
        {
            let mut inner = self.inner.lock().unwrap();
            if inner.hash.as_deref() == Some(&clip.hash) {
                return;
            }
            inner.hash = Some(clip.hash.clone());
            inner.at = clip.at;
            inner.pending = None;
        }
        let bytes = clip.text.len();
        let data = serde_json::to_value(&clip).unwrap();
        if daemon.sessions.event("clipboard", data) {
            tracing::info!("clipboard: laptop → phone ({bytes} bytes)");
            daemon.event("clipboard", serde_json::json!({"from": "laptop", "bytes": bytes}));
        } else {
            self.inner.lock().unwrap().pending = Some(clip);
        }
    }

    /// Sends a clip made while disconnected. The phone keeps it only if it
    /// is newer than its own.
    pub fn on_connected(&self, daemon: &Shared) {
        let pending = self.inner.lock().unwrap().pending.take();
        if let Some(clip) = pending {
            daemon.sessions.event("clipboard", serde_json::to_value(&clip).unwrap());
        }
    }

    /// `phonectl clip`: push the current clipboard now, even if unchanged.
    pub async fn push_now(&self, daemon: &Shared) -> Result<(), String> {
        let text = read().await.map_err(|e| e.to_string())?.ok_or("clipboard has no text")?;
        self.inner.lock().unwrap().hash = None;
        self.local(daemon, text);
        Ok(())
    }
}

/// Watches for clipboard changes forever (restarting wl-paste if it dies,
/// e.g. when the compositor restarts).
pub async fn watch(daemon: Shared) {
    let mut first = true;
    loop {
        if let Err(e) = watch_once(&daemon, &mut first).await {
            tracing::warn!("clipboard watch: {e}");
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn watch_once(daemon: &Shared, first: &mut bool) -> std::io::Result<()> {
    let mut child = Command::new("wl-paste")
        .args(["--watch", "sh", "-c", "cat >/dev/null; echo"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    loop {
        let Some(_) = lines.next_line().await? else { break };
        // wl-paste fires once at start for the current clipboard. Only treat
        // that as "ours" so a reconnect can push it, never as a new copy.
        let debounce = tokio::time::sleep(Duration::from_millis(300));
        tokio::pin!(debounce);
        loop {
            tokio::select! {
                _ = &mut debounce => break,
                next = lines.next_line() => if next?.is_none() { return Ok(()) },
            }
        }
        match read().await {
            Ok(Some(text)) if *first => {
                *first = false;
                let mut inner = daemon.clipboard.inner.lock().unwrap();
                if inner.hash.is_none() {
                    inner.hash = Some(link::clip_hash(&text));
                }
            }
            Ok(Some(text)) => daemon.clipboard.local(daemon, text),
            Ok(None) => *first = false,
            Err(e) => tracing::debug!("clipboard read: {e}"),
        }
    }
    let _ = child.wait().await;
    Ok(())
}

/// The clipboard text, or None for non-text or secret content.
async fn read() -> std::io::Result<Option<String>> {
    let types = Command::new("wl-paste").arg("--list-types").stderr(Stdio::null()).output().await?;
    let types = String::from_utf8_lossy(&types.stdout).to_string();
    if types.lines().any(|t| t == "x-kde-passwordManagerHint") {
        let hint = Command::new("wl-paste").args(["--type", "x-kde-passwordManagerHint"]).stderr(Stdio::null()).output().await?;
        if String::from_utf8_lossy(&hint.stdout).trim() == "secret" {
            return Ok(None);
        }
    }
    let mime = ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain", "TEXT", "STRING"]
        .into_iter()
        .find(|m| types.lines().any(|t| t == *m));
    let Some(mime) = mime else { return Ok(None) };
    let out = Command::new("wl-paste").args(["--no-newline", "--type", mime]).stderr(Stdio::null()).output().await?;
    if !out.status.success() || out.stdout.is_empty() || out.stdout.len() > link::MAX_CLIP {
        return Ok(None);
    }
    Ok(String::from_utf8(out.stdout).ok())
}

async fn write(text: &str) -> std::io::Result<()> {
    let mut child = Command::new("wl-copy")
        .args(["--type", "text/plain;charset=utf-8"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(text.as_bytes()).await?;
    drop(stdin);
    // wl-copy forks a server and exits once it owns the selection.
    let status = child.wait().await?;
    if status.success() { Ok(()) } else { Err(std::io::Error::other(format!("wl-copy exited with {status}"))) }
}
