//! Pauses laptop media while the phone rings or is in a call, and resumes
//! exactly what we paused once the call ends.
//!
//! Talks MPRIS over the session bus directly (the same interface playerctl
//! uses). `playerctld` is skipped: it proxies other players, so pausing it
//! too would pause one player twice.
//!
//! Rules:
//!  - only players that were Playing when the call started are paused
//!  - on hang-up, only those are resumed, and only if they are still Paused
//!    (if you stopped or started something yourself meanwhile, we keep out)
//!  - a player that appears playing during the call is paused too (and
//!    resumed after)

use std::sync::Mutex;

use zbus::fdo::DBusProxy;
use zbus::proxy::CacheProperties;

#[zbus::proxy(interface = "org.mpris.MediaPlayer2.Player", default_path = "/org/mpris/MediaPlayer2")]
trait Player {
    fn pause(&self) -> zbus::Result<()>;
    fn play(&self) -> zbus::Result<()>;
    #[zbus(property)]
    fn playback_status(&self) -> zbus::Result<String>;
}

pub struct CallMedia {
    connection: zbus::Connection,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    in_call: bool,
    paused: Vec<String>,
}

impl CallMedia {
    pub async fn new() -> anyhow::Result<Self> {
        Ok(CallMedia { connection: zbus::Connection::session().await?, inner: Mutex::default() })
    }

    /// `state`: idle | ringing | offhook. Idempotent: status snapshots repeat it.
    pub async fn call_state(&self, state: &str) {
        let active = state == "ringing" || state == "offhook";
        let was = std::mem::replace(&mut self.inner.lock().unwrap().in_call, active);
        if active && !was {
            let paused = self.pause_playing().await;
            tracing::info!("call {state}: paused {} player(s)", paused.len());
            self.inner.lock().unwrap().paused = paused;
        } else if active {
            // Still in the call: catch anything that started playing since.
            let more = self.pause_playing().await;
            self.inner.lock().unwrap().paused.extend(more);
        } else if was {
            let paused = std::mem::take(&mut self.inner.lock().unwrap().paused);
            let mut resumed = 0;
            for name in paused {
                if let Ok(player) = self.player(&name).await
                    && player.playback_status().await.as_deref() == Ok("Paused")
                    && player.play().await.is_ok()
                {
                    resumed += 1;
                }
            }
            tracing::info!("call ended: resumed {resumed} player(s)");
        }
    }

    async fn pause_playing(&self) -> Vec<String> {
        let mut paused = Vec::new();
        let Ok(dbus) = DBusProxy::new(&self.connection).await else { return paused };
        let Ok(names) = dbus.list_names().await else { return paused };
        for name in names {
            let name = name.to_string();
            if !name.starts_with("org.mpris.MediaPlayer2.") || name == "org.mpris.MediaPlayer2.playerctld" {
                continue;
            }
            let Ok(player) = self.player(&name).await else { continue };
            if player.playback_status().await.as_deref() == Ok("Playing") && player.pause().await.is_ok() {
                paused.push(name);
            }
        }
        paused
    }

    async fn player(&self, name: &str) -> zbus::Result<PlayerProxy<'static>> {
        PlayerProxy::builder(&self.connection)
            .destination(name.to_owned())?
            .cache_properties(CacheProperties::No)
            .build()
            .await
    }
}
