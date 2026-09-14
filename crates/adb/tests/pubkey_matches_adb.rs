//! Cross-checks our Android public-key encoding against the real `adb pubkey`.
//!
//! Development-only: `adb` is not a runtime dependency of phonectl. The test
//! skips when the binary is missing.

use std::io::Write;
use std::process::Command;

use adb::HostKey;

fn adb() -> Option<String> {
    for candidate in ["adb", concat!(env!("HOME"), "/sdk/android/platform-tools/adb")] {
        if Command::new(candidate).arg("version").output().is_ok() {
            return Some(candidate.to_string());
        }
    }
    None
}

#[test]
fn our_public_key_matches_adb_pubkey() {
    let Some(adb) = adb() else {
        eprintln!("skipping: no adb binary");
        return;
    };

    let key = HostKey::generate().unwrap();
    let mut pem = tempfile::NamedTempFile::new().unwrap();
    pem.write_all(key.to_pem().unwrap().as_bytes()).unwrap();
    pem.flush().unwrap();

    let out = Command::new(adb).arg("pubkey").arg(pem.path()).output().unwrap();
    assert!(out.status.success(), "adb pubkey failed: {}", String::from_utf8_lossy(&out.stderr));

    let theirs = String::from_utf8(out.stdout).unwrap();
    let theirs = theirs.trim_end_matches(['\n', '\0']).split(' ').next().unwrap().to_string();

    let ours = String::from_utf8(key.public_key_payload("test@host")).unwrap();
    let ours = ours.trim_end_matches('\0').split(' ').next().unwrap().to_string();

    assert_eq!(ours, theirs);
}
