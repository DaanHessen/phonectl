//! Where phonectl keeps its files, following the XDG base directory spec.

use std::path::PathBuf;

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

fn xdg(variable: &str, fallback: &str) -> PathBuf {
    match std::env::var_os(variable) {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => home().join(fallback),
    }
}

/// `~/.config/phonectl`
pub fn config_dir() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join("phonectl")
}

/// `~/.local/share/phonectl`: the ADB key and known devices live here.
pub fn data_dir() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share").join("phonectl")
}

/// `$XDG_RUNTIME_DIR/phonectl`: the daemon socket lives here.
pub fn runtime_dir() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(value) if !value.is_empty() => PathBuf::from(value).join("phonectl"),
        _ => std::env::temp_dir().join(format!("phonectl-{}", users_uid())),
    }
}

fn users_uid() -> u32 {
    // Safe: getuid cannot fail and touches no memory we own.
    unsafe { libc_getuid() }
}

unsafe extern "C" {
    #[link_name = "getuid"]
    fn libc_getuid() -> u32;
}

pub fn adb_key() -> PathBuf {
    data_dir().join("adbkey")
}

pub fn devices_file() -> PathBuf {
    data_dir().join("devices.toml")
}

pub fn config_file() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn daemon_socket() -> PathBuf {
    runtime_dir().join("daemon.sock")
}

/// How our key is labelled on the phone, e.g. `daan@omarchy`.
pub fn key_comment() -> String {
    let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
    let host = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|h| h.trim().to_string())
        .unwrap_or_else(|_| "linux".into());
    format!("{user}@{host}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xdg_variables_win_when_set() {
        // Safe in a single-threaded test; these are process-wide.
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", "/tmp/cfg");
            std::env::set_var("XDG_DATA_HOME", "/tmp/data");
            std::env::set_var("XDG_RUNTIME_DIR", "/tmp/run");
        }
        assert_eq!(config_file(), PathBuf::from("/tmp/cfg/phonectl/config.toml"));
        assert_eq!(adb_key(), PathBuf::from("/tmp/data/phonectl/adbkey"));
        assert_eq!(daemon_socket(), PathBuf::from("/tmp/run/phonectl/daemon.sock"));
    }

    #[test]
    fn key_comment_looks_like_user_at_host() {
        let comment = key_comment();
        assert!(comment.contains('@'), "{comment}");
        assert!(!comment.starts_with('@'), "{comment}");
        assert!(!comment.ends_with('@'), "{comment}");
    }
}
