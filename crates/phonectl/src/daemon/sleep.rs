//! Suspend and resume, from logind.
//!
//! Before suspend we hold a short delay inhibitor so we can tell the phone
//! "sleeping": it then stops retrying (saving its battery) until we poke it
//! after resume. TCP over Tailscale would often survive a short suspend, but
//! a clean close plus a poke is faster and predictable.

use std::time::Duration;

use futures::StreamExt;
use phone::link::Message;

use super::Shared;

#[zbus::proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1"
)]
trait Manager {
    fn inhibit(&self, what: &str, who: &str, why: &str, mode: &str) -> zbus::Result<zbus::zvariant::OwnedFd>;
    #[zbus(signal)]
    fn prepare_for_sleep(&self, start: bool) -> zbus::Result<()>;
}

pub async fn watch(daemon: Shared) {
    if let Err(e) = run(daemon).await {
        tracing::warn!("suspend handling unavailable: {e}");
    }
}

async fn run(daemon: Shared) -> zbus::Result<()> {
    let connection = zbus::Connection::system().await?;
    let manager = ManagerProxy::new(&connection).await?;
    let mut signals = manager.receive_prepare_for_sleep().await?;
    let mut lock = inhibit(&manager).await;
    while let Some(signal) = signals.next().await {
        let Ok(args) = signal.args() else { continue };
        if args.start {
            tracing::info!("suspending: telling the phone");
            daemon.state.lock().unwrap().asleep = true;
            if daemon.sessions.send(Message::Sleeping) {
                daemon.sessions.close_current().await;
                // Give the write a moment to leave before the lid shuts.
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
            daemon.publish();
            drop(lock.take());
        } else {
            tracing::info!("resumed");
            daemon.state.lock().unwrap().asleep = false;
            daemon.publish();
            lock = inhibit(&manager).await;
            tokio::spawn(super::net::poke_burst(daemon.clone(), "resume"));
        }
    }
    Ok(())
}

async fn inhibit(manager: &ManagerProxy<'_>) -> Option<zbus::zvariant::OwnedFd> {
    match manager.inhibit("sleep", "phonectl", "Tell the phone the laptop is suspending", "delay").await {
        Ok(fd) => Some(fd),
        Err(e) => {
            tracing::warn!("no sleep inhibitor: {e}");
            None
        }
    }
}
