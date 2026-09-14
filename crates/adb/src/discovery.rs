//! Finding phones on the local network.
//!
//! Wireless debugging picks a fresh TCP port every time it is enabled, so the
//! address can only come from mDNS. Android advertises:
//!
//! - `_adb-tls-connect._tcp` while wireless debugging is on and a host is
//!   paired, instance name `adb-<serial>-<suffix>`
//! - `_adb-tls-pairing._tcp` only while the pairing dialog is open
//! - `_adb._tcp` for the legacy (unencrypted) service, which we ignore
//!
//! We browse with an in-process responder (`mdns-sd`) rather than talking to
//! Avahi, so phonectl has no daemon dependency.

use std::net::SocketAddr;
use std::time::Duration;

use tokio::sync::mpsc;

use crate::error::{Error, Result};

pub const SERVICE_CONNECT: &str = "_adb-tls-connect._tcp.local.";
pub const SERVICE_PAIRING: &str = "_adb-tls-pairing._tcp.local.";

/// A phone seen on the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovered {
    /// mDNS instance name, e.g. `adb-0123456789ABCDE-aBcDeF`. Stable across
    /// reboots, so this is what we match a known phone on.
    pub instance: String,
    pub address: SocketAddr,
    pub service: Service,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Service {
    Connect,
    Pairing,
}

impl Service {
    pub fn service_type(self) -> &'static str {
        match self {
            Self::Connect => SERVICE_CONNECT,
            Self::Pairing => SERVICE_PAIRING,
        }
    }
}

impl Discovered {
    /// The device serial embedded in `adb-<serial>-<suffix>`, when it has that
    /// shape.
    pub fn serial(&self) -> Option<&str> {
        self.instance.strip_prefix("adb-")?.rsplit_once('-').map(|(serial, _)| serial)
    }
}

/// Watches for phones until the returned receiver is dropped.
///
/// Every appearance is reported, including ones seen again after the phone
/// changes address, which is exactly the signal the daemon needs to reconnect.
pub fn watch(service: Service) -> Result<mpsc::Receiver<Discovered>> {
    let daemon = mdns_sd::ServiceDaemon::new()
        .map_err(|e| Error::Discovery(format!("cannot start mDNS: {e}")))?;
    let receiver = daemon
        .browse(service.service_type())
        .map_err(|e| Error::Discovery(format!("cannot browse {}: {e}", service.service_type())))?;

    let (tx, rx) = mpsc::channel(16);
    std::thread::spawn(move || {
        // Keep the daemon alive for as long as anyone is listening.
        let _daemon = daemon;
        while let Ok(event) = receiver.recv() {
            let mdns_sd::ServiceEvent::ServiceResolved(info) = event else { continue };
            let Some(found) = resolve(&info, service) else { continue };
            if tx.blocking_send(found).is_err() {
                break;
            }
        }
    });
    Ok(rx)
}

fn resolve(info: &mdns_sd::ResolvedService, service: Service) -> Option<Discovered> {
    let instance = info.fullname.split('.').next()?.to_string();
    // Prefer IPv4: adbd listens on both, and v4 avoids link-local scope ids.
    let mut addresses: Vec<std::net::IpAddr> = info.addresses.iter().map(|a| a.to_ip_addr()).collect();
    addresses.sort_by_key(|a| !a.is_ipv4());
    let address = SocketAddr::new(*addresses.first()?, info.port);
    Some(Discovered { instance, address, service })
}

/// Waits for one phone to show up, or gives up after `timeout`.
///
/// `instance` restricts the search to one phone; `None` takes the first seen.
pub async fn find(service: Service, instance: Option<&str>, timeout: Duration) -> Result<Discovered> {
    let mut rx = watch(service)?;
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Some(found)) => {
                if instance.is_none_or(|wanted| wanted == found.instance) {
                    return Ok(found);
                }
            }
            Ok(None) => return Err(Error::Discovery("mDNS browser stopped".into())),
            Err(_) => return Err(Error::NotFound),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use super::*;

    fn discovered(instance: &str) -> Discovered {
        Discovered {
            instance: instance.to_string(),
            address: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 2, 15)), 39615),
            service: Service::Connect,
        }
    }

    #[test]
    fn service_types_match_androids() {
        assert_eq!(Service::Connect.service_type(), "_adb-tls-connect._tcp.local.");
        assert_eq!(Service::Pairing.service_type(), "_adb-tls-pairing._tcp.local.");
    }

    #[test]
    fn serial_is_extracted_from_the_instance_name() {
        // The real instance name seen from the Phone (4a) Pro.
        assert_eq!(discovered("adb-0123456789ABCDE-aBcDeF").serial(), Some("0123456789ABCDE"));
        assert_eq!(discovered("something-else").serial(), None);
        assert_eq!(discovered("adb-noserial").serial(), None);
    }

    #[tokio::test]
    async fn find_times_out_when_nothing_answers() {
        let err = find(Service::Pairing, Some("adb-nope"), Duration::from_millis(150)).await.unwrap_err();
        assert!(matches!(err, Error::NotFound), "{err}");
    }
}
