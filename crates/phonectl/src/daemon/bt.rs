//! Bluetooth fallback transport: an RFCOMM profile registered with BlueZ.
//!
//! The laptop never scans or dials. It only registers the service record;
//! the phone connects to it (as a bonded device) when it has no IP path. An
//! idle registered profile costs nothing: the adapter is connectable anyway.

use std::time::Duration;

use futures::StreamExt;
use phone::link;

use super::{Shared, Transport};

pub async fn serve(daemon: Shared) {
    let mut delay = Duration::from_secs(5);
    loop {
        match register(&daemon).await {
            Ok(()) => delay = Duration::from_secs(5),
            Err(e) => tracing::warn!("bluetooth fallback unavailable: {e}"),
        }
        daemon.state.lock().unwrap().bluetooth = false;
        daemon.publish();
        // bluetoothd restarted or is not running yet.
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(Duration::from_secs(600));
    }
}

async fn register(daemon: &Shared) -> bluer::Result<()> {
    let session = bluer::Session::new().await?;
    let profile = bluer::rfcomm::Profile {
        uuid: link::BT_UUID.parse().expect("valid UUID"),
        name: Some("phonectl".into()),
        role: Some(bluer::rfcomm::Role::Server),
        require_authentication: Some(true),
        require_authorization: Some(false),
        auto_connect: Some(false),
        ..Default::default()
    };
    let mut handle = session.register_profile(profile).await?;
    tracing::info!("bluetooth fallback profile registered");
    daemon.state.lock().unwrap().bluetooth = true;
    daemon.publish();

    while let Some(request) = handle.next().await {
        let device = request.device();
        match request.accept() {
            Ok(stream) => {
                tracing::info!("bluetooth connection from a bonded device");
                tokio::spawn(super::session::run(daemon.clone(), stream, Transport::Bluetooth, device.to_string()));
            }
            Err(e) => tracing::warn!("bluetooth accept: {e}"),
        }
    }
    Ok(())
}
