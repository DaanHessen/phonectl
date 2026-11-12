//! Do Not Disturb, both ways. The laptop side is the notification server's
//! "do-not-disturb" mode on mako's D-Bus interface (`fr.emersion.Mako`,
//! served by mako or by latent-bar's built-in server); the phone side is
//! `status.dnd` and the phone method `dnd {on}`.
//!
//! Only changes are mirrored, never the state found at startup or at a
//! reconnect, so neither side overrides the other by just showing up. A
//! change made on one side is pushed to the other unless that side already
//! has it, which also stops the echo of our own change from bouncing back.

use std::sync::Mutex;
use std::time::Duration;

use futures::StreamExt;
use serde_json::{Value, json};

use super::Shared;

const DND: &str = "do-not-disturb";

#[zbus::proxy(
    interface = "fr.emersion.Mako",
    default_service = "org.freedesktop.Notifications",
    default_path = "/fr/emersion/Mako"
)]
trait Mako {
    fn list_modes(&self) -> zbus::Result<Vec<String>>;
    fn set_modes(&self, modes: &[&str]) -> zbus::Result<()>;
    #[zbus(property)]
    fn modes(&self) -> zbus::Result<Vec<String>>;
}

#[derive(Default)]
pub struct DndSync {
    inner: Mutex<Sides>,
}

/// Last state seen on each side; `None` until the first reading.
#[derive(Default, Debug, PartialEq, Eq)]
pub struct Sides {
    pub laptop: Option<bool>,
    pub phone: Option<bool>,
}

/// What to do after a reading.
#[derive(Debug, PartialEq, Eq)]
pub enum Push {
    None,
    ToPhone(bool),
    ToLaptop(bool),
}

impl Sides {
    /// A reading from the laptop. Pushes a real change to the phone if the
    /// phone is known and differs.
    pub fn laptop_says(&mut self, on: bool) -> Push {
        let before = self.laptop.replace(on);
        match (before, self.phone) {
            (Some(b), Some(p)) if b != on && p != on => Push::ToPhone(on),
            _ => Push::None,
        }
    }

    /// A reading from the phone (every status snapshot).
    pub fn phone_says(&mut self, on: bool) -> Push {
        let before = self.phone.replace(on);
        match (before, self.laptop) {
            (Some(b), Some(l)) if b != on && l != on => Push::ToLaptop(on),
            _ => Push::None,
        }
    }

    /// The phone went away: its next reading is a fresh baseline.
    pub fn phone_gone(&mut self) {
        self.phone = None;
    }

    /// We set the phone ourselves: what it reports next is not a change of its own.
    pub fn phone_set(&mut self, on: bool) {
        self.phone = Some(on);
    }
}

impl DndSync {
    pub fn on_phone_status(&self, daemon: &Shared, status: &Value) {
        if !daemon.config.dnd_sync {
            return;
        }
        let Some(on) = status.get("dnd").and_then(Value::as_bool) else { return };
        if let Push::ToLaptop(on) = self.inner.lock().unwrap().phone_says(on) {
            tokio::spawn(async move {
                match set_laptop(on).await {
                    Ok(()) => tracing::info!("do not disturb {} (from the phone)", if on { "on" } else { "off" }),
                    Err(e) => tracing::warn!("do not disturb: could not set the laptop: {e}"),
                }
            });
        }
    }

    pub fn on_disconnected(&self) {
        self.inner.lock().unwrap().phone_gone();
    }

    fn on_laptop(&self, daemon: &Shared, on: bool) {
        let push = self.inner.lock().unwrap().laptop_says(on);
        if let Push::ToPhone(on) = push {
            let daemon = daemon.clone();
            tokio::spawn(async move {
                match daemon.sessions.call("dnd", json!({"on": on})).await {
                    Ok(_) => {
                        daemon.dnd.inner.lock().unwrap().phone_set(on);
                        tracing::info!("do not disturb {} (to the phone)", if on { "on" } else { "off" });
                    }
                    Err(e) => tracing::warn!("do not disturb: phone refused: {e}"),
                }
            });
        }
    }
}

async fn set_laptop(on: bool) -> zbus::Result<()> {
    let connection = zbus::Connection::session().await?;
    let mako = MakoProxy::new(&connection).await?;
    let mut modes = mako.list_modes().await?;
    modes.retain(|m| m != DND);
    if on {
        modes.push(DND.into());
    }
    if modes.is_empty() {
        modes.push("default".into());
    }
    let refs: Vec<&str> = modes.iter().map(String::as_str).collect();
    mako.set_modes(&refs).await
}

/// Follows the laptop's mode: change signals, plus a slow poll in case the
/// notification server restarts and the signal subscription goes stale.
pub async fn watch(daemon: Shared) {
    if !daemon.config.dnd_sync {
        return;
    }
    loop {
        if let Err(e) = follow(&daemon).await {
            tracing::debug!("do not disturb: notification server not reachable: {e}");
        }
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
}

async fn follow(daemon: &Shared) -> zbus::Result<()> {
    let connection = zbus::Connection::session().await?;
    let mako = MakoProxy::new(&connection).await?;
    let mut changes = mako.receive_modes_changed().await;
    let mut poll = tokio::time::interval(Duration::from_secs(30));
    loop {
        tokio::select! {
            change = changes.next() => {
                let Some(change) = change else { return Ok(()) };
                if let Ok(modes) = change.get().await {
                    daemon.dnd.on_laptop(daemon, modes.iter().any(|m| m == DND));
                }
            }
            _ = poll.tick() => {
                let modes = mako.list_modes().await?;
                daemon.dnd.on_laptop(daemon, modes.iter().any(|m| m == DND));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Push, Sides};

    #[test]
    fn startup_and_reconnect_readings_are_not_changes() {
        let mut s = Sides::default();
        assert_eq!(s.laptop_says(true), Push::None);
        assert_eq!(s.phone_says(false), Push::None);
        s.phone_gone();
        assert_eq!(s.phone_says(false), Push::None);
    }

    #[test]
    fn a_change_on_one_side_reaches_the_other_once() {
        let mut s = Sides::default();
        s.laptop_says(false);
        s.phone_says(false);
        assert_eq!(s.laptop_says(true), Push::ToPhone(true));
        s.phone_set(true);
        // The phone's next status shows our own change: no echo.
        assert_eq!(s.phone_says(true), Push::None);
        assert_eq!(s.phone_says(false), Push::ToLaptop(false));
        // The laptop then reports what we set: no echo either.
        assert_eq!(s.laptop_says(false), Push::None);
    }

    #[test]
    fn nothing_is_pushed_when_the_other_side_already_agrees() {
        let mut s = Sides::default();
        s.laptop_says(true);
        s.phone_says(false);
        assert_eq!(s.phone_says(true), Push::None);
        assert_eq!(s.laptop_says(true), Push::None);
    }
}
