//! Pairing key and one-time phone setup.
//!
//! `phonectl setup` creates the link key, prints the pairing string, and, if
//! the phone is reachable over ADB right now, installs the app and grants
//! everything that would otherwise need a trip through Settings. ADB is only
//! used here: once setup is done, USB/wireless debugging can be turned off
//! (some payment apps refuse to run while it is on). The grants persist.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::process::Command;

use base64::Engine;

use crate::config::Config;

pub const PACKAGE: &str = "com.daanh.phonectl";

pub fn key_file() -> PathBuf {
    phone::paths::data_dir().join("link.key")
}

pub fn load_key() -> anyhow::Result<Vec<u8>> {
    let path = key_file();
    let text = std::fs::read_to_string(&path)
        .map_err(|_| anyhow::anyhow!("no pairing key at {}; run `phonectl setup` first", path.display()))?;
    let key = base64::engine::general_purpose::STANDARD.decode(text.trim())?;
    anyhow::ensure!(key.len() >= 16, "pairing key in {} is too short", path.display());
    Ok(key)
}

fn create_key() -> anyhow::Result<Vec<u8>> {
    let mut key = vec![0u8; 32];
    rand::Rng::fill(&mut rand::thread_rng(), &mut key[..]);
    let path = key_file();
    std::fs::create_dir_all(path.parent().unwrap())?;
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)?;
    file.write_all(base64::engine::general_purpose::STANDARD.encode(&key).as_bytes())?;
    Ok(key)
}

pub fn pairing_string(config: &Config, key: &[u8], host: &str, bt: Option<&str>) -> String {
    let mut s = format!("phonectl:1;host={host};port={};name={}", config.port, config.name);
    if let Some(bt) = bt {
        s += &format!(";bt={bt}");
    }
    s + &format!(";key={}", base64::engine::general_purpose::STANDARD.encode(key))
}

async fn bluetooth_address() -> Option<String> {
    let session = bluer::Session::new().await.ok()?;
    let adapter = session.default_adapter().await.ok()?;
    adapter.address().await.ok().map(|a| a.to_string())
}

pub struct Options {
    pub new_key: bool,
    pub apk: Option<PathBuf>,
    pub no_adb: bool,
}

pub async fn run(config: &Config, options: Options) -> anyhow::Result<()> {
    let key = match (options.new_key, load_key()) {
        (false, Ok(key)) => key,
        (true, _) => {
            let _ = std::fs::remove_file(key_file());
            create_key()?
        }
        (false, Err(_)) => create_key()?,
    };
    let host = crate::daemon::net::tailscale_address()
        .ok_or_else(|| anyhow::anyhow!("Tailscale is not up on this laptop (no 100.x address)"))?
        .to_string();
    let bt = bluetooth_address().await;
    let pairing = pairing_string(config, &key, &host, bt.as_deref());

    if options.no_adb || !adb_ready() {
        if !options.no_adb {
            println!("No phone on ADB; skipping install and grants.");
        }
        println!("Paste this into the phonectl app (Pairing):\n\n{pairing}\n");
        println!("Keep it private: it is the key to the link.");
        return Ok(());
    }

    let apk = options.apk.or_else(default_apk);
    match &apk {
        Some(apk) => {
            println!("installing {}", apk.display());
            adb(&["install", "-r", &apk.to_string_lossy()])?;
        }
        None => println!("no APK found (build it: cd android && ./gradlew assembleRelease); assuming it is installed"),
    }

    println!("granting permissions");
    let grants = [
        format!("pm grant {PACKAGE} android.permission.READ_LOGS"),
        format!("pm grant {PACKAGE} android.permission.READ_PHONE_STATE"),
        format!("pm grant {PACKAGE} android.permission.BLUETOOTH_CONNECT"),
        format!("pm grant {PACKAGE} android.permission.POST_NOTIFICATIONS"),
        format!("appops set {PACKAGE} SYSTEM_ALERT_WINDOW allow"),
        // `phonectl update` installs new versions over the link.
        format!("appops set {PACKAGE} REQUEST_INSTALL_PACKAGES allow"),
        format!("cmd notification allow_listener {PACKAGE}/{PACKAGE}.NotifListener"),
        // Ringer "silent" from the laptop menu needs Do Not Disturb access.
        format!("cmd notification allow_dnd {PACKAGE}"),
        format!("dumpsys deviceidle whitelist +{PACKAGE}"),
        // Sideloaded apps get "restricted settings" on Android 15+; the
        // grants above bypass it, this keeps the toggles usable in Settings.
        format!("appops set {PACKAGE} ACCESS_RESTRICTED_SETTINGS allow"),
    ];
    for grant in &grants {
        if let Err(e) = adb(&["shell", grant]) {
            println!("  warning: `{grant}` failed: {e}");
        }
    }
    // Restart so the clipboard log watcher starts with READ_LOGS in hand.
    let _ = adb(&["shell", &format!("am force-stop {PACKAGE}")]);
    adb(&[
        "shell",
        &format!("am start -n {PACKAGE}/.MainActivity --es pair '{pairing}'"),
    ])?;
    println!("paired. The app is open on the phone; anything still marked ✗ there needs a tap.");
    println!("Once it shows Connected you can turn off Wireless/USB debugging again.");
    Ok(())
}

pub fn default_apk() -> Option<PathBuf> {
    let candidates = [
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../android/app/build/outputs/apk/release/app-release.apk"),
        phone::paths::data_dir().join("phonectl.apk"),
    ];
    candidates.into_iter().find(|p| p.exists()).map(|p| p.canonicalize().unwrap_or(p))
}

fn adb_ready() -> bool {
    Command::new("adb")
        .args(["get-state"])
        .output()
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "device")
        .unwrap_or(false)
}

fn adb(args: &[&str]) -> anyhow::Result<()> {
    let out = Command::new("adb").args(args).output()?;
    let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    if !out.status.success() || text.contains("Exception") || text.contains("Failure") {
        anyhow::bail!("{}", text.trim());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_string_shape() {
        let config = Config { name: "omarchy".into(), port: 47201, dismiss_on_phone: true };
        let s = pairing_string(&config, &[0u8; 32], "100.1.2.3", Some("00:11:22:33:44:55"));
        assert!(s.starts_with("phonectl:1;host=100.1.2.3;port=47201;name=omarchy;bt=00:11:22:33:44:55;key="));
        assert!(!s.contains(' '));
    }
}
