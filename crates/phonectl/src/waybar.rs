//! `phonectl waybar`: the Waybar module. Long-running, no `interval`: it
//! subscribes to the daemon and prints one JSON line whenever the status
//! changes. Nothing runs per refresh.
//!
//! Looks like the earctl/sonyctl modules: icon + battery %, charging bolt,
//! classes offline/connecting/low/critical/charging, and the shared tooltip
//! sheet (see ~/.config/waybar/scripts/tooltip-lib.sh).

use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const ICON_PHONE: &str = "\u{f011c}"; // md-cellphone
const ICON_BLUETOOTH: &str = "\u{f0815}"; // md-cellphone-wireless
const ICON_OFF: &str = "\u{f0950}"; // md-cellphone-off
const ICON_CALL: &str = "\u{f0952}"; // md-cellphone-sound
const ICON_CHARGING: &str = "\u{f140b}"; // md-lightning-bolt (same as earctl)
const LOW: i64 = 20;
const CRITICAL: i64 = 10;

pub async fn run() -> anyhow::Result<()> {
    let mut stdout = tokio::io::stdout();
    let mut delay = Duration::from_secs(2);
    let mut last = String::new();
    loop {
        if let Ok(stream) = tokio::net::UnixStream::connect(phone::paths::daemon_socket()).await {
            delay = Duration::from_secs(2);
            let (read, mut write) = stream.into_split();
            write.write_all(b"{\"method\":\"subscribe\"}\n").await?;
            let mut lines = BufReader::new(read).lines();
            let theme = Theme::load();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
                if message.get("type").and_then(Value::as_str) != Some("status") {
                    continue;
                }
                let out = render(&message["data"], &theme).to_string();
                if out != last {
                    stdout.write_all(format!("{out}\n").as_bytes()).await?;
                    stdout.flush().await?;
                    last = out;
                }
            }
        }
        let out = json!({"text": ICON_OFF, "class": ["offline"], "tooltip": "phonectl daemon is not running\n<span alpha=\"50%\">systemctl --user enable --now phonectl</span>"}).to_string();
        if out != last {
            stdout.write_all(format!("{out}\n").as_bytes()).await?;
            stdout.flush().await?;
            last = out;
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(Duration::from_secs(30));
    }
}

pub struct Theme {
    accent: String,
    warn: String,
    crit: String,
}

impl Theme {
    pub fn load() -> Theme {
        let home = std::env::var("HOME").unwrap_or_default();
        let text = std::fs::read_to_string(format!("{home}/.config/omarchy/current/theme/colors.toml")).unwrap_or_default();
        let get = |key: &str, fallback: &str| -> String {
            text.lines()
                .filter_map(|l| l.split_once('='))
                .find(|(k, _)| k.trim() == key)
                .map(|(_, v)| v.trim().trim_matches('"').to_string())
                .filter(|v| v.len() == 7 && v.starts_with('#'))
                .unwrap_or_else(|| fallback.to_string())
        };
        Theme { accent: get("accent", "#81a1c1"), warn: get("color3", "#ebcb8b"), crit: get("color1", "#bf616a") }
    }
}

fn dim(s: &str) -> String {
    format!("<span alpha=\"50%\">{s}</span>")
}

fn row(label: &str, value: &str) -> String {
    dim(&format!("{label:<8}")) + value
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn bars(level: Option<i64>) -> String {
    let Some(level) = level else { return String::new() };
    let glyphs = ["▂", "▄", "▆", "█"];
    let lit: String = glyphs[..level.clamp(0, 4) as usize].concat();
    let unlit: String = glyphs[level.clamp(0, 4) as usize..].concat();
    format!("{lit}{}", if unlit.is_empty() { String::new() } else { dim(&unlit) })
}

fn ago(seconds: i64) -> String {
    let delta = (super::daemon::now_millis() / 1000 - seconds).max(0);
    match delta {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", delta / 60),
        3600..86400 => format!("{} h ago", delta / 3600),
        _ => format!("{} days ago", delta / 86400),
    }
}

pub fn render(s: &Value, theme: &Theme) -> Value {
    let state = s["state"].as_str().unwrap_or("disconnected");
    if state == "unpaired" {
        return json!({"text": "", "class": ["hidden"], "tooltip": ""});
    }
    let status = &s["status"];
    let level = status["battery"]["level"].as_i64();
    let charging = status["battery"]["charging"].as_bool().unwrap_or(false);
    let full = status["battery"]["full"].as_bool().unwrap_or(false);
    let call = status["call"].as_str().unwrap_or("idle");
    let connected = state == "connected";
    let transport = s["transport"].as_str();

    let icon = if !connected {
        ICON_OFF
    } else if call != "idle" {
        ICON_CALL
    } else if transport == Some("bluetooth") {
        ICON_BLUETOOTH
    } else {
        ICON_PHONE
    };
    let mut text = icon.to_string();
    if connected && let Some(level) = level {
        text += &format!(" {level}%");
        if charging {
            text += &format!(" {ICON_CHARGING}");
        }
    }

    let mut class: Vec<&str> = Vec::new();
    match state {
        "connected" => class.push(if transport == Some("bluetooth") { "bluetooth" } else { "connected" }),
        "connecting" => class.push("connecting"),
        _ => class.push("offline"),
    }
    if connected && let Some(level) = level {
        if level <= CRITICAL {
            class.push("critical");
        } else if level <= LOW {
            class.push("low");
        }
    }
    if connected && charging {
        class.push("charging");
    }
    if connected && call != "idle" {
        class.push("call");
    }

    // Tooltip, in the shared sheet layout.
    let name = s["phone"]["name"].as_str().unwrap_or("Phone");
    let detail = match state {
        "connected" => transport.unwrap_or("connected").to_string(),
        "connecting" => "connecting…".into(),
        "suspended" => "laptop suspended".into(),
        _ => "offline".into(),
    };
    let mut lines = vec![format!("<b><span color=\"{}\">{}</span></b>{}", theme.accent, esc(name), dim(&format!(" · {detail}")))];

    if !status.is_null() {
        lines.push(format!("\n{}", dim(if connected { "status" } else { "last known" })));
        if let Some(level) = level {
            let pct = if level <= CRITICAL {
                format!("<span color=\"{}\">{level}%</span>", theme.crit)
            } else if level <= LOW {
                format!("<span color=\"{}\">{level}%</span>", theme.warn)
            } else {
                format!("{level}%")
            };
            let plug = status["battery"]["plugged"].as_str();
            let how = if full {
                dim(" · full")
            } else if charging {
                dim(&format!(" · charging{}", plug.map(|p| format!(" ({p})")).unwrap_or_default()))
            } else {
                String::new()
            };
            lines.push(row("battery", &(pct + &how)));
        }
        let net = &status["network"];
        let network = match net["type"].as_str() {
            Some("wifi") => format!("wi-fi {}", bars(net["wifi_level"].as_i64())),
            Some("cellular") => format!("{} {}", net["cellular"].as_str().unwrap_or("mobile"), bars(net["cellular_level"].as_i64())),
            Some("none") | None => dim("no network"),
            Some(other) => other.to_string(),
        };
        lines.push(row("network", &network));
        // Mobile signal is worth showing even on Wi-Fi (calls, SMS).
        if net["type"].as_str() != Some("cellular") && net["cellular_level"].is_i64() {
            let carrier = net["operator"].as_str().map(|o| dim(&format!(" · {}", esc(o)))).unwrap_or_default();
            lines.push(row("mobile", &format!("{} {}{carrier}", net["cellular"].as_str().unwrap_or(""), bars(net["cellular_level"].as_i64())).trim_start()));
        } else if let Some(op) = net["operator"].as_str() {
            lines.push(row("carrier", &esc(op)));
        }
        if call != "idle" {
            lines.push(row("call", if call == "ringing" { "ringing" } else { "in a call" }));
        }
    }
    let notifications = s["notifications"].as_i64().unwrap_or(0);
    if connected && notifications > 0 {
        lines.push(row("notifs", &format!("{notifications} on the phone")));
    }

    lines.push(format!("\n{}", dim("link")));
    match state {
        "connected" => {
            let since = s["connected_since"].as_i64().map(|t| dim(&format!(" · since {}", ago(t).replace(" ago", "")))).unwrap_or_default();
            lines.push(row("via", &format!("{}{since}", transport.unwrap_or("?"))));
        }
        _ => {
            if let Some(seen) = s["last_seen"].as_i64() {
                lines.push(row("seen", &ago(seen)));
            }
            if let Some(error) = s["error"].as_str() {
                lines.push(row("last", &dim(&esc(error))));
            }
        }
    }
    if s["laptop"]["listening"] == false {
        lines.push(row("laptop", &format!("<span color=\"{}\">Tailscale is down</span>", theme.warn)));
    }
    if let Some(perms) = status["permissions"].as_object() {
        let missing: Vec<&str> = [
            ("notifications", "notification access"),
            ("phone_state", "phone state"),
            ("clipboard_auto", "auto clipboard"),
            ("bluetooth", "bluetooth"),
        ]
        .into_iter()
        .filter(|(k, _)| perms.get(*k) == Some(&Value::Bool(false)))
        .map(|(_, label)| label)
        .collect();
        if !missing.is_empty() {
            lines.push(row("setup", &dim(&format!("missing {}", missing.join(", ")))));
        }
    }
    lines.push(format!("\n{}", dim("click menu · right send clipboard · middle reconnect")));

    json!({
        "text": text,
        "class": class,
        "percentage": level.unwrap_or(0),
        "tooltip": format!("<span line_height=\"1.4\">{}</span>", lines.join("\n")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> Theme {
        Theme { accent: "#111111".into(), warn: "#222222".into(), crit: "#333333".into() }
    }

    fn connected(level: i64, charging: bool) -> Value {
        json!({
            "state": "connected", "transport": "tailscale", "connected_since": 0, "notifications": 2,
            "phone": {"name": "Nothing Phone (4a) Pro"},
            "laptop": {"listening": true},
            "status": {
                "battery": {"level": level, "charging": charging, "full": false, "plugged": if charging { json!("usb") } else { Value::Null }},
                "network": {"type": "cellular", "cellular": "5G", "cellular_level": 3, "wifi_level": null, "operator": "KPN"},
                "call": "idle",
                "permissions": {"notifications": true, "phone_state": true, "clipboard_auto": true, "bluetooth": true}
            }
        })
    }

    #[test]
    fn connected_shows_battery_and_network() {
        let out = render(&connected(84, false), &theme());
        assert_eq!(out["text"], format!("{ICON_PHONE} 84%"));
        assert_eq!(out["class"], json!(["connected"]));
        let tooltip = out["tooltip"].as_str().unwrap();
        assert!(tooltip.contains("5G ▂▄▆"), "{tooltip}");
        assert!(tooltip.contains("KPN"));
        assert!(!tooltip.contains("missing"));
    }

    #[test]
    fn charging_and_low_classes() {
        let out = render(&connected(15, true), &theme());
        assert_eq!(out["text"], format!("{ICON_PHONE} 15% {ICON_CHARGING}"));
        assert_eq!(out["class"], json!(["connected", "low", "charging"]));
        let out = render(&connected(5, false), &theme());
        assert_eq!(out["class"], json!(["connected", "critical"]));
    }

    #[test]
    fn offline_and_unpaired() {
        let mut s = connected(50, false);
        s["state"] = json!("disconnected");
        s["transport"] = Value::Null;
        s["last_seen"] = json!(0);
        let out = render(&s, &theme());
        assert_eq!(out["text"], ICON_OFF);
        assert_eq!(out["class"], json!(["offline"]));
        assert!(out["tooltip"].as_str().unwrap().contains("last known"));

        let out = render(&json!({"state": "unpaired"}), &theme());
        assert_eq!(out["class"], json!(["hidden"]));
    }

    #[test]
    fn bluetooth_and_call() {
        let mut s = connected(60, false);
        s["transport"] = json!("bluetooth");
        assert_eq!(render(&s, &theme())["text"], format!("{ICON_BLUETOOTH} 60%"));
        s["status"]["call"] = json!("ringing");
        let out = render(&s, &theme());
        assert_eq!(out["text"], format!("{ICON_CALL} 60%"));
        assert!(out["class"].as_array().unwrap().contains(&json!("call")));
    }
}
