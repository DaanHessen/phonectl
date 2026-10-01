> **Superseded (2026-10-01).** This ADB + shell-agent design cannot work off
> Wi-Fi (wireless debugging is Wi-Fi only) and conflicts with apps that refuse
> to run while debugging is on. phonectl is now an Android app talking to the
> laptop daemon over Tailscale, with Bluetooth as fallback; see README.md.
> Kept for the research it records.

# Architecture

Status: draft 1, 2026-09-14. Built on `docs/research/REPORT.md`; every major
decision below points back to a finding there.

## Shape

```
 phone (uid 2000, no APK)                    laptop (user session)
┌──────────────────────────┐   ADB/TLS    ┌───────────────────────────────┐
│ agent.dex (app_process)  │◀────────────▶│ phonectl daemon             │
│  notifications listener  │  one stream  │  adb transport + pairing      │
│  media sessions          │  JSON lines  │  discovery (mDNS)             │
│  battery (binder poll)   │              │  connection state machine     │
│  lights / Glyph          │              │  device model + capabilities  │
│  settings, clipboard     │              │  IPC server (Unix socket)     │
└──────────────────────────┘              └──────────────┬────────────────┘
                                                         │ JSON lines
                                   ┌─────────────────────┼──────────────┐
                                   │                     │              │
                             phonectl CLI     phonectl waybar   other clients
                                              (push stream, no poll)
```

## Crates

| Crate | Kind | Knows about | Must not know about |
|---|---|---|---|
| `adb` | lib | ADB wire protocol, TLS, pairing, mDNS service names | phones, Nothing, agent, IPC |
| `phone` | lib | device model, capabilities, agent protocol, connection state machine, config | Waybar, CLI, sockets to clients |
| `phonectl` | bin | daemon wiring, IPC server + client, CLI, Waybar renderer, menu | wire formats of ADB |
| `agent/` | Java | Android framework internals | anything on the laptop except the agent protocol |

The earctl lessons this layout acts on:

- earctl mixes transport, device logic and the HTTP server in one crate. Here
  the ADB layer is its own crate, testable without a phone, and could be
  published separately.
- earctl exposes an HTTP port on 127.0.0.1, which every local user and every
  browser tab can reach. Here IPC is a Unix socket in `$XDG_RUNTIME_DIR`.
- The earctl/sonyctl Waybar scripts poll every 15 s with curl + jq and several
  bluetoothctl calls. Here Waybar gets a push stream from the daemon and
  nothing runs per refresh.
- earctl has no event stream. Here events are first-class: the agent pushes,
  the daemon fans out, `phonectl events` prints them.

## adb crate

Own async implementation on tokio. `adb_client` is used as a reference, not a
dependency (REPORT §2: blocking, single stream, no pairing).

- `message`: 24-byte header codec, CRC skipping per `A_VERSION_SKIP_CHECKSUM`,
  payload limits negotiated in CNXN.
- `auth`: RSA 2048 key (`rsa`), ADB public-key encoding, AUTH token signing.
- `tls`: STLS upgrade, rustls TLS 1.3 client with a self-signed cert from the
  ADB key (rcgen). The server cert is not CA-verified (adbd uses self-signed
  certs); trust comes from the pairing, as in AOSP.
- `transport`: a connection task owns the socket; streams are multiplexed by
  local id; each `Stream` is an `AsyncRead + AsyncWrite` with OKAY-based flow
  control. Opening `shell,v2,raw:…`, `localabstract:…`, `sync:` are all just
  `open(service)`.
- `pair`: TLS 1.3 → SPAKE2 (BoringSSL spake25519 variant, on
  curve25519-dalek) → HKDF-SHA256 → AES-128-GCM → PeerInfo exchange. Verified
  against the real phone; unit-tested with vectors captured from that run.
- `mdns`: browse `_adb-tls-connect._tcp` and `_adb-tls-pairing._tcp` with
  `mdns-sd` (in-process, no Avahi dependency).
- `usb` (later): `nusb`, pure Rust, async.

## Phone agent

Java, compiled against `android-36` with javac + d8 into a single dex. The
built dex is committed (`agent/dist/agent.dex`) so building phonectl needs no
Android SDK; `agent/build.sh` rebuilds it and CI checks it is up to date. The
daemon embeds it with `include_bytes!`.

Lifecycle:

1. The daemon computes the dex hash and pushes it to
   `/data/local/tmp/phonectl/agent-<hash>.dex` over `sync:` if missing, with
   a fresh random token file (mode 0600, shell-owned; apps cannot read
   `/data/local/tmp`).
2. It starts the agent detached (`setsid`), so a Wi-Fi blip does not kill it.
3. The agent listens on the abstract socket `phonectl` and accepts one
   client that presents the token.
4. After 15 minutes without a client the agent exits. Nothing lingers if the
   laptop never returns. Reboot also clears it.

Inside the agent (all verified in the spikes, REPORT §5):

- System context from `ActivityThread`, wrapped to identify as
  `com.android.shell`
- Notifications: `NotificationListenerService.registerAsSystemService`
- Media: `ISessionManager` + `MediaController` callbacks
- Battery: `BatteryManager` binder getters on a `Handler` timer (uptime-based:
  it never wakes the phone), default 60 s, event emitted only on change
- Lights: `ILightsManager` session (pending the write test)
- Settings: Glyph toggles, DND, Glyph Progress
- Clipboard: `IClipboard` listener

## Agent protocol

Newline-delimited JSON, one object per line, UTF-8. It is easy to debug with
`nc`, the volume is tiny, and serde handles it on the Rust side.

```
→ {"type":"hello","token":"…","protocol":1}
← {"type":"hello","protocol":1,"agent":"<hash>","device":{…},"capabilities":["battery",…]}
→ {"type":"call","id":7,"method":"media.action","params":{"action":"play_pause"}}
← {"type":"result","id":7,"ok":true,"data":null}
← {"type":"event","topic":"battery","data":{"level":43,"charging":false,…}}
```

Capabilities are probed on the phone at startup (does `lights` exist, do the
Glyph settings keys exist, etc.), never assumed from the model name.

## Connection state machine

```
Disconnected ──mDNS/config──▶ Connecting ──TLS ok──▶ StartingAgent ──hello──▶ Connected
     ▲                         │  TLS cert rejected                  │ agent fails
     │                         ▼                                     ▼
     └──backoff── Reconnecting ◀── PairingRequired             Degraded (ADB up, agent down)
```

The public state enum is `disconnected | discovering | pairing_required |
connecting | connected | degraded | reconnecting`.

Rules:

- One driver task owns the state; everything else sends it events. No shared
  mutable state, so there are no races between reconnect paths.
- Backoff 1 s doubling to 5 min, with jitter. It resets on an mDNS announcement
  for our phone, on resume from suspend (logind `PrepareForSleep(false)`), or
  on `phonectl connect`.
- Before suspend (`PrepareForSleep(true)`) the daemon closes the connection
  cleanly; the agent's idle timer covers the rest.
- TCP keepalive (idle 120 s, interval 30 s, 4 probes) detects dead links
  without app-level pings.
- The address comes from mDNS, never from config. A cached last address is
  tried first, then discarded if it fails.

## Device model

```rust
pub enum Capability { Battery, Notifications, Media, Clipboard, Dnd, DeviceInfo,
                      Network, Screenshot, FindPhone, Glyph, GlyphSettings,
                      ReverseCharging }
```

`DeviceProfile` = generic Android + optional vendor extension. The Nothing
extension is active when the agent reports the Nothing capabilities (Glyph
lights present, `led_effect_enable` exists), so another Android phone gets the
generic set and nothing breaks. The abstraction boundary is only where the
phone showed a real difference; no speculative vendor traits.

## Linux IPC

`$XDG_RUNTIME_DIR/phonectl/daemon.sock`, directory mode 0700.
Newline-delimited JSON, like lumend's socket but async:

```
→ {"id":1,"method":"status"}
← {"id":1,"ok":true,"data":{…Status…}}
→ {"id":2,"method":"subscribe","params":{"topics":["state","battery","media"]}}
← {"type":"event","topic":"battery",…}   (until the client disconnects)
```

`Status` is the stable, versioned schema (`"schema": 1`) that Waybar and
scripts use. Its serde form is the contract, covered by snapshot tests.

## CLI

```
phonectl status [--json]          device, connection, battery, media, notifications
phonectl devices                  paired + discovered phones
phonectl pair [CODE] [--address]  pair via mDNS-found or given pairing service
phonectl connect | disconnect
phonectl battery [--json]
phonectl notifications [list|dismiss KEY] [--json]
phonectl media [status|play|pause|toggle|next|previous]
phonectl clipboard [get|set TEXT]
phonectl glyph [test|off|brightness N|…]
phonectl find                     ring the phone
phonectl screenshot [PATH]
phonectl capabilities
phonectl events [--json]          live event stream
phonectl waybar                   Waybar module output (continuous)
phonectl menu                     walker/wofi control menu
phonectl daemon                   run the daemon (systemd unit uses this)
```

Exit codes: 0 ok, 1 generic error, 2 usage, 3 daemon not running,
4 phone not connected, 5 capability not supported, 6 pairing required.
Errors state what to do next ("wireless debugging is off on the phone: enable
it in Developer options").

## Waybar

Same conventions as `custom/earctl` / `custom/sonyctl` (inspected in
`~/.config/waybar`):

- `custom/phonectl`, `return-type: json`, **no `interval`**: `phonectl
  waybar` streams one JSON line per change
- Hidden (`class: hidden`, empty text) when no phone is paired or it is away
- Text: phone glyph + battery % + charging bolt, like the headphone modules
- Classes: `offline`, `connecting`, `low` (≤20), `critical` (≤10),
  `charging`; the CSS block copies `#custom-earctl`'s
- Tooltip: bold name, then aligned plain-text rows (Battery, Now playing,
  Notifications, Network, Glyph), then the `<small>` click-help footer
- Left click: media play/pause; middle: find phone; right: `phonectl menu`
  (walker → wofi fallback, ●/○ marks, same `pick()` behaviour as the scripts)

## Configuration

`~/.config/phonectl/config.toml`, optional, validated on load with clear
errors. Pairing state lives in `~/.local/share/phonectl/` (`adbkey` 0600,
`devices.toml`).

## Power and resource budget

| Where | Cost | How it is kept down |
|---|---|---|
| Phone CPU | ~0 when idle | agent blocks in Looper; events only |
| Phone wakeups | none added | uptime timers only; no alarms or wakelocks |
| Phone RAM | one small JVM (to be measured) | single-dex agent, no Kotlin/AndroidX |
| Network | idle TLS socket + keepalive every ~2 min | no app pings |
| Laptop | one daemon, no subprocesses | Waybar push stream; no adb binary |

Anything that would need polling (battery) runs on the phone, on the uptime
clock, and only emits changes.

## Testing

- `adb`: codec round-trips, CRC, STLS handshake against a fake adbd, stream
  flow control, pairing crypto with recorded vectors
- `phone`: state machine driven by a fake transport with tokio paused time
  (no sleeps), agent protocol serde round-trips, capability detection
- `phonectl`: IPC against a temp socket, CLI via `assert_cmd`, Waybar render
  golden tests
- live tests behind `--ignored` + `NOTHINGCTL_LIVE=1`, run against the real
  phone
