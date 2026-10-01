//! The laptop side of the link: one long-running user service.
//!
//! Tasks (all event-driven; nothing polls):
//!  - `net`: TCP listener on the Tailscale address, UDP pokes to the phone,
//!    and a netlink watch that pokes when the laptop's network changes
//!  - `bt`: RFCOMM profile registered with BlueZ (fallback transport)
//!  - `session`: one authenticated connection at a time, whichever transport
//!  - `clipboard`, `notify`, `media`: the features
//!  - `sleep`: logind suspend/resume
//!  - `ipc`: Unix socket for the CLI and the Waybar module

pub mod bt;
pub mod clipboard;
pub mod ipc;
pub mod media;
pub mod net;
pub mod notify;
pub mod session;
pub mod sleep;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use phone::link::DeviceInfo;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{broadcast, watch};

use crate::config::Config;

pub fn now_millis() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// What we remember about the paired phone across restarts.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PhoneRecord {
    pub device: DeviceInfo,
    pub tailscale_ip: Option<String>,
    pub app_version: Option<String>,
    /// Last status the phone sent, so Waybar can show "last seen" data.
    pub status: Option<Value>,
    pub last_seen: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    Tailscale,
    Bluetooth,
}

#[derive(Debug, Default)]
pub struct State {
    pub transport: Option<Transport>,
    pub connected_since: Option<i64>,
    /// Set after we poked the phone; shown as "connecting" until it expires.
    pub connecting_until: Option<Instant>,
    pub phone: PhoneRecord,
    pub error: Option<String>,
    pub notifications: usize,
    pub listening: bool,
    pub bluetooth: bool,
    pub asleep: bool,
}

pub struct Daemon {
    pub config: Config,
    pub key: Vec<u8>,
    pub state: Mutex<State>,
    pub status_tx: watch::Sender<Value>,
    pub events: broadcast::Sender<Value>,
    pub sessions: session::Registry,
    pub clipboard: clipboard::Clipboard,
    pub notifier: notify::Notifier,
    pub media: media::CallMedia,
}

pub type Shared = Arc<Daemon>;

impl Daemon {
    /// Recomputes the public status and wakes every subscriber if it changed.
    pub fn publish(&self) {
        let status = self.status();
        self.status_tx.send_if_modified(|current| {
            if *current == status {
                false
            } else {
                *current = status;
                true
            }
        });
    }

    pub fn status(&self) -> Value {
        let state = self.state.lock().unwrap();
        let connecting = state.connecting_until.is_some_and(|until| until > Instant::now());
        let paired = state.phone.last_seen.is_some() || !state.phone.device.model.is_empty();
        let label = if state.transport.is_some() {
            "connected"
        } else if state.asleep {
            "suspended"
        } else if connecting {
            "connecting"
        } else if !paired {
            "unpaired"
        } else {
            "disconnected"
        };
        json!({
            "schema": 1,
            "state": label,
            "transport": state.transport,
            "connected_since": state.connected_since,
            "last_seen": state.phone.last_seen,
            "error": state.error,
            "laptop": {"listening": state.listening, "bluetooth": state.bluetooth},
            "phone": {
                "name": if paired { Value::String(state.phone.device.display_name()) } else { Value::Null },
                "model": state.phone.device.model,
                "android": state.phone.device.android_version,
                "tailscale_ip": state.phone.tailscale_ip,
                "app_version": state.phone.app_version,
            },
            "status": state.phone.status,
            "notifications": state.notifications,
        })
    }

    pub fn event(&self, topic: &str, data: Value) {
        let _ = self.events.send(json!({"type": "event", "topic": topic, "data": data}));
    }

    pub fn save_phone(&self) {
        let record = self.state.lock().unwrap().phone.clone();
        if let Ok(text) = serde_json::to_string_pretty(&record) {
            let path = phone_file();
            let _ = std::fs::create_dir_all(path.parent().unwrap());
            let _ = std::fs::write(path, text);
        }
    }

    /// Marks "connecting" for a while after a poke, so Waybar shows progress.
    pub fn expect_connection(self: &Arc<Self>, within: Duration) {
        self.state.lock().unwrap().connecting_until = Some(Instant::now() + within);
        self.publish();
        let me = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(within + Duration::from_millis(50)).await;
            me.publish();
        });
    }
}

pub fn phone_file() -> PathBuf {
    phone::paths::data_dir().join("phone.json")
}

fn load_phone() -> PhoneRecord {
    std::fs::read_to_string(phone_file())
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub async fn run(config: Config) -> anyhow::Result<()> {
    let key = crate::setup::load_key()?;
    let (status_tx, _) = watch::channel(Value::Null);
    let (events, _) = broadcast::channel(64);
    let notifier = notify::Notifier::new(&config).await?;
    let daemon = Arc::new(Daemon {
        clipboard: clipboard::Clipboard::new(),
        media: media::CallMedia::new().await?,
        notifier,
        sessions: session::Registry::default(),
        state: Mutex::new(State { phone: load_phone(), ..State::default() }),
        key,
        config,
        status_tx,
        events,
    });
    daemon.publish();

    let ipc = ipc::serve(daemon.clone()).await?;
    tokio::spawn(net::listen(daemon.clone()));
    tokio::spawn(net::watch_network(daemon.clone()));
    tokio::spawn(bt::serve(daemon.clone()));
    tokio::spawn(clipboard::watch(daemon.clone()));
    tokio::spawn(notify::watch_signals(daemon.clone()));
    tokio::spawn(sleep::watch(daemon.clone()));

    // The phone may be backing off after our restart: tell it we are here.
    net::poke_burst(daemon.clone(), "startup").await;

    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        _ = term.recv() => {}
        _ = tokio::signal::ctrl_c() => {}
    }
    tracing::info!("shutting down");
    daemon.notifier.save();
    drop(ipc);
    Ok(())
}
