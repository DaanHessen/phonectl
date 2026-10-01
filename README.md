# phonectl

My Nothing Phone (4a) Pro, wired into my Omarchy/Hyprland laptop: clipboard
both ways, phone notifications in mako, laptop media paused during calls, and
a Waybar module + drop-down panel with the phone's status. It works on any
network, because it runs over the Tailscale network both devices are already
on, with Bluetooth as a fallback when there is no network at all.

## Pieces

| Where | What |
|---|---|
| `android/` | The phone app (Kotlin, no AndroidX, ~110 KB). Runs inside its notification listener process, so no foreground service and no permanent notification. |
| `crates/phonectl` | Laptop daemon + CLI + Waybar module (`phonectl daemon`, run by `contrib/phonectl.service`). |
| `crates/phone` | Shared protocol (`link.rs`), paths, and the ADB connection used for development. |
| `crates/adb` | Own ADB implementation (TLS, pairing, mDNS). Only for setup/development now. |
| `~/.local/libexec/bar-panel` (dotfiles) | The `phone` drop-down panel. |

## Transport

1. **Tailscale TCP** (laptop listens on its 100.x address, port 47201, nothing
   else). The phone always dials; Tailscale handles Wi-Fi ↔ 5G, different
   networks and NAT, so a session usually survives network changes.
2. **Bluetooth RFCOMM** (bonded, encrypted) when the phone has no network or
   Tailscale fails twice. While on Bluetooth the phone retries Tailscale on
   every network change and every 10 minutes, then drops Bluetooth.
3. Wi-Fi Direct/Aware were not used: the laptop runs iwd (no P2P group support
   to speak of), and Android needs a user prompt per Wi-Fi Direct connection.
   The phone's hotspot already gives an IP path, which Tailscale then uses.

Both ends authenticate with an HMAC challenge over a 32-byte key from
`phonectl setup` (stored in `~/.local/share/phonectl/link.key`, mode 0600, and
in app-private storage on the phone).

Reconnection is event-driven: phone network callbacks, a UDP "poke" the laptop
sends on startup/resume/network change, and backoff timers on the uptime clock
(2 s doubling to 10 min; they stop while the phone sleeps). When the laptop
suspends it tells the phone, which then only retries every 30 min until poked.
The laptop pings after 4 min of silence; the phone drops a link silent for
13 min.

## Setup (once)

```sh
cd android && ./gradlew assembleRelease && cd ..
cargo build --release
ln -sf "$PWD/target/release/phonectl" ~/.local/bin/phonectl
systemctl --user enable --now phonectl        # unit: contrib/phonectl.service
# Phone: Developer options > Wireless debugging on, paired with `adb pair`
phonectl setup        # installs the app, grants permissions, pairs
```

Then turn Wireless debugging (and Developer options) off again; nothing needs
ADB afterwards. Updates go over the link: `phonectl update`.

Bluetooth fallback additionally needs the phone and laptop paired once in
Bluetooth settings.

## Commands

```
phonectl status [--json]    phonectl events        phonectl diag
phonectl clip               phonectl connect       phonectl update
phonectl ring [--stop]      phonectl ringer MODE   phonectl media ACTION
phonectl test-notification  phonectl waybar        phonectl setup
```

## Known limits (Android, not bugs)

- **Automatic phone → laptop clipboard needs one tap after every phone reboot
  or app update.** Android 10+ only lets the focused app read the clipboard.
  phonectl watches the system log for the moment the clipboard changes and
  briefly focuses an invisible activity to read it (what KDE Connect does).
  Android 13+ shows a log-access consent dialog for that, only to foreground
  apps, so after a restart the app posts a quiet "Clipboard sync to laptop is
  paused" notification; tap it and allow. Until then: share sheet → "Send to
  laptop", the Quick Settings tile, or opening the app. Android shows its own
  "phonectl pasted from your clipboard" toast on each read.
- Reply actions (inline text replies) are not offered on the laptop; mako has
  no text input. Other notification actions and "open on phone" work.
- Passwords copied from password managers (marked sensitive) are not synced,
  in either direction.
