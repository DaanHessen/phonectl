//! Phone notifications as native desktop notifications (mako, via
//! org.freedesktop.Notifications).
//!
//! - app name and icon are the phone app's (icons fetched once per app and
//!   cached in ~/.cache/phonectl/icons)
//! - updates replace the same desktop notification instead of stacking
//! - removal on the phone closes it here
//! - clicking (mako's default action) opens it on the phone; plain actions
//!   ("Mark as read", "Decline") are offered as notification actions
//! - dismissing it here dismisses it on the phone (`dismiss_on_phone`)
//!
//! The key → id map is kept in $XDG_RUNTIME_DIR so a daemon restart neither
//! duplicates nor orphans what is on screen.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use base64::Engine;
use futures::StreamExt;
use phone::link::Notification;
use serde_json::json;
use zbus::zvariant::Value as ZValue;

use super::Shared;
use crate::config::Config;

#[zbus::proxy(
    interface = "org.freedesktop.Notifications",
    default_service = "org.freedesktop.Notifications",
    default_path = "/org/freedesktop/Notifications"
)]
trait Notifications {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: &[&str],
        hints: HashMap<&str, ZValue<'_>>,
        expire_timeout: i32,
    ) -> zbus::Result<u32>;

    fn close_notification(&self, id: u32) -> zbus::Result<()>;

    #[zbus(signal)]
    fn action_invoked(&self, id: u32, action_key: String) -> zbus::Result<()>;

    #[zbus(signal)]
    fn notification_closed(&self, id: u32, reason: u32) -> zbus::Result<()>;
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Shown {
    /// phone key → desktop id
    ids: HashMap<String, u32>,
    /// phone key → clearable on the phone
    clearable: HashMap<String, bool>,
    /// phone key → content fingerprint of everything shown since it was
    /// posted, including ones mako has since expired. Keeps a reconnect from
    /// popping up old notifications again.
    #[serde(default)]
    seen: HashMap<String, String>,
}

fn fingerprint(n: &Notification) -> String {
    format!("{}\u{0}{}", n.title.as_deref().unwrap_or(""), n.text.as_deref().unwrap_or(""))
}

pub struct Notifier {
    proxy: NotificationsProxy<'static>,
    shown: Mutex<Shown>,
    dismiss_on_phone: bool,
    /// Ids we closed ourselves, so their Closed signal is not echoed back.
    closing: Mutex<Vec<u32>>,
}

const REASON_DISMISSED: u32 = 2;
/// Notifications older than this are not popped up after a (re)connect.
const STALE_MS: i64 = 10 * 60 * 1000;

impl Notifier {
    pub async fn new(config: &Config) -> anyhow::Result<Self> {
        let connection = zbus::Connection::session().await?;
        let proxy = NotificationsProxy::new(&connection).await?;
        let shown = std::fs::read_to_string(map_file())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Ok(Notifier { proxy, shown: Mutex::new(shown), dismiss_on_phone: config.dismiss_on_phone, closing: Mutex::new(Vec::new()) })
    }

    pub fn save(&self) {
        let text = serde_json::to_string(&*self.shown.lock().unwrap()).unwrap_or_default();
        let path = map_file();
        let _ = std::fs::create_dir_all(path.parent().unwrap());
        let _ = std::fs::write(path, text);
    }

    fn count(&self) -> usize {
        self.shown.lock().unwrap().seen.len()
    }

    pub async fn show(&self, daemon: &Shared, n: Notification) {
        let icon = icon_path(daemon, &n.package).await;
        let replaces = self.shown.lock().unwrap().ids.get(&n.key).copied().unwrap_or(0);
        let is_call = n.category.as_deref() == Some("call");

        let mut actions: Vec<String> = Vec::new();
        if n.can_open {
            actions.extend(["default".into(), "Open on phone".into()]);
        }
        for action in n.actions.iter().filter(|a| !a.reply) {
            actions.push(format!("a{}", action.index));
            actions.push(action.title.clone());
        }
        let actions: Vec<&str> = actions.iter().map(String::as_str).collect();

        let mut hints: HashMap<&str, ZValue<'_>> = HashMap::new();
        hints.insert("urgency", ZValue::U8(if is_call { 2 } else { 1 }));
        if n.silent || n.update {
            hints.insert("suppress-sound", ZValue::Bool(true));
        }
        hints.insert("x-phonectl", ZValue::Bool(true));

        let app = n.app.clone().unwrap_or_else(|| n.package.clone());
        let summary = n.title.clone().unwrap_or_else(|| app.clone());
        let body = escape(n.text.as_deref().unwrap_or(""));
        let timeout = if is_call { 0 } else { -1 };
        match self.proxy.notify(&app, replaces, &icon, &summary, &body, &actions, hints, timeout).await {
            Ok(id) => {
                {
                    let mut shown = self.shown.lock().unwrap();
                    shown.ids.insert(n.key.clone(), id);
                    shown.clearable.insert(n.key.clone(), n.clearable);
                    shown.seen.insert(n.key.clone(), fingerprint(&n));
                }
                self.save();
                self.publish_count(daemon);
                daemon.event("notification", json!({"app": app, "update": n.update}));
            }
            Err(e) => tracing::warn!("cannot show notification: {e}"),
        }
    }

    pub async fn remove(&self, daemon: &Shared, key: &str) {
        let id = {
            let mut shown = self.shown.lock().unwrap();
            shown.clearable.remove(key);
            shown.seen.remove(key);
            shown.ids.remove(key)
        };
        self.save();
        if let Some(id) = id {
            self.closing.lock().unwrap().push(id);
            let _ = self.proxy.close_notification(id).await;
            self.save();
            self.publish_count(daemon);
        }
    }

    /// After a (re)connect the phone sends everything it currently has.
    /// Close what is gone; show what is new or changed. Anything already on
    /// screen with the same key is updated in place (no new popup sound).
    pub async fn reconcile(&self, daemon: &Shared, items: Vec<Notification>) {
        let keys: std::collections::HashSet<&str> = items.iter().map(|n| n.key.as_str()).collect();
        let gone: Vec<String> = {
            let shown = self.shown.lock().unwrap();
            shown.ids.keys().chain(shown.seen.keys()).filter(|k| !keys.contains(k.as_str())).cloned().collect()
        };
        for key in gone {
            self.remove(daemon, &key).await;
        }
        for mut n in items {
            let (seen, on_screen) = {
                let shown = self.shown.lock().unwrap();
                (shown.seen.get(&n.key).cloned(), shown.ids.contains_key(&n.key))
            };
            // Never seen and already old (first pairing, or posted long
            // before this reconnect): remember it, but do not pop it up.
            if seen.is_none() && !on_screen && n.posted_at.is_some_and(|t| super::now_millis() - t > STALE_MS) {
                self.shown.lock().unwrap().seen.insert(n.key.clone(), fingerprint(&n));
                continue;
            }
            match seen {
                Some(fp) if fp == fingerprint(&n) => continue,
                // Changed while we were apart: update quietly in place.
                Some(_) => n.update = true,
                None => {}
            }
            if on_screen {
                n.update = true;
            }
            self.show(daemon, n).await;
        }
        self.save();
        self.publish_count(daemon);
    }

    fn publish_count(&self, daemon: &Shared) {
        daemon.state.lock().unwrap().notifications = self.count();
        daemon.publish();
    }

    fn key_for(&self, id: u32) -> Option<String> {
        self.shown.lock().unwrap().ids.iter().find(|(_, v)| **v == id).map(|(k, _)| k.clone())
    }
}

/// Reacts to clicks and dismissals in mako.
pub async fn watch_signals(daemon: Shared) {
    let notifier = &daemon.notifier;
    let (Ok(mut invoked), Ok(mut closed)) =
        (notifier.proxy.receive_action_invoked().await, notifier.proxy.receive_notification_closed().await)
    else {
        tracing::warn!("cannot subscribe to notification signals");
        return;
    };
    loop {
        tokio::select! {
            Some(signal) = invoked.next() => {
                let Ok(args) = signal.args() else { continue };
                let Some(key) = notifier.key_for(args.id) else { continue };
                let (method, params) = if args.action_key == "default" {
                    ("notification_open", json!({"key": key}))
                } else if let Some(index) = args.action_key.strip_prefix('a').and_then(|i| i.parse::<u32>().ok()) {
                    ("notification_action", json!({"key": key, "index": index}))
                } else {
                    continue;
                };
                let daemon = daemon.clone();
                tokio::spawn(async move {
                    if let Err(e) = daemon.sessions.call(method, params).await {
                        tracing::info!("{method} failed: {e}");
                    }
                });
            }
            Some(signal) = closed.next() => {
                let Ok(args) = signal.args() else { continue };
                let ours = {
                    let mut closing = notifier.closing.lock().unwrap();
                    match closing.iter().position(|id| *id == args.id) {
                        Some(i) => { closing.remove(i); true }
                        None => false,
                    }
                };
                if ours { continue; }
                let Some(key) = notifier.key_for(args.id) else { continue };
                let clearable = {
                    let mut shown = notifier.shown.lock().unwrap();
                    shown.ids.remove(&key);
                    shown.clearable.remove(&key).unwrap_or(false)
                };
                notifier.save();
                notifier.publish_count(&daemon);
                if args.reason == REASON_DISMISSED && clearable && notifier.dismiss_on_phone {
                    let daemon = daemon.clone();
                    tokio::spawn(async move {
                        let _ = daemon.sessions.call("notification_dismiss", json!({"key": key})).await;
                    });
                }
            }
            else => return,
        }
    }
}

/// Cached app icon, fetched from the phone on first use.
async fn icon_path(daemon: &Shared, package: &str) -> String {
    if package.is_empty() || !package.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_') {
        return "phone".into();
    }
    let path = icon_dir().join(format!("{package}.png"));
    if path.exists() {
        return path.to_string_lossy().into_owned();
    }
    if let Ok(data) = daemon.sessions.call("icon", json!({"package": package})).await
        && let Some(png) = data.get("png").and_then(|v| v.as_str())
        && let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(png)
    {
        let _ = std::fs::create_dir_all(icon_dir());
        if std::fs::write(&path, bytes).is_ok() {
            return path.to_string_lossy().into_owned();
        }
    }
    "phone".into()
}

fn icon_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache"));
    base.join("phonectl/icons")
}

fn map_file() -> PathBuf {
    phone::paths::runtime_dir().join("notifications.json")
}

/// mako renders Pango markup in bodies.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    #[test]
    fn markup_is_escaped() {
        assert_eq!(super::escape("a<b> & c"), "a&lt;b&gt; &amp; c");
    }
}
