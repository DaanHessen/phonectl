//! `~/.config/phonectl/config.toml`, all optional:
//!
//! ```toml
//! name = "omarchy"          # how the phone shows this laptop
//! port = 47201              # TCP port on the Tailscale address
//! dismiss_on_phone = true   # dismissing a mirrored notification here clears it on the phone
//! ```

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub name: String,
    pub port: u16,
    pub dismiss_on_phone: bool,
}

impl Default for Config {
    fn default() -> Self {
        let name = std::fs::read_to_string("/proc/sys/kernel/hostname")
            .map(|h| h.trim().to_string())
            .unwrap_or_else(|_| "laptop".into());
        Config { name, port: phone::link::TCP_PORT, dismiss_on_phone: true }
    }
}

impl Config {
    pub fn load() -> anyhow::Result<Config> {
        let path = phone::paths::config_file();
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).map_err(|e| anyhow::anyhow!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(anyhow::anyhow!("{}: {e}", path.display())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_config_keeps_defaults() {
        let config: Config = toml::from_str("dismiss_on_phone = false").unwrap();
        assert!(!config.dismiss_on_phone);
        assert_eq!(config.port, 47201);
        assert!(toml::from_str::<Config>("prot = 1").is_err(), "typos are rejected");
    }
}
