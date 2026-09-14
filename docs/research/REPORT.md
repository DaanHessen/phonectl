# Research report

Date: 2026-09-14. This is the synthesis. The detailed track reports are
alongside it:

- `ecosystem-kdeconnect.md`: KDE Connect, GSConnect, Valent, scrcpy, other
  Linux/Android tools
- `nothing-specific.md`: Glyph, Nothing OS, Nothing apps
- `adb-connection-rust.md`: wireless debugging, discovery, Rust crates
- `device-observations.md`: what the real phone showed (the spikes)

Confidence tags: **[verified]** = checked on the phone or in source code,
**[documented]** = stated by primary docs, **[inferred]** = reasoning from
verified facts, **[open]** = not yet known.

## 1. The device

The phone is a **Nothing Phone (4a) Pro** (A069P, FroggerPro, SM7750,
Android 16 / SDK 36, Nothing OS 4.1), not the Phone 2 from the brief
[verified]. That matters twice. Android 16 is new enough for the Live Updates
to Glyph Progress path, and some research about the Phone 2's sysfs Glyph
driver does not apply.

## 2. Ecosystem map

| Project | What it gives | Use in phonectl |
|---|---|---|
| KDE Connect / GSConnect / Valent | Mature phone-desktop protocol over TLS on port 1716: battery, notifications, media, clipboard, SMS, find-phone. Needs its own Android app. | **Not used at runtime** (Daan's rule). Reference only. Its TCP keepalive every 10 s is linked to phone battery drain in KDE bug 442782 and MR !447 [documented]; our design avoids that. |
| scrcpy (Apache-2.0) | Screen, audio, camera, control. Pushes a Java server to `/data/local/tmp` and runs it with `app_process` as the shell user. | **Its technique is the core of our phone agent.** We do not depend on scrcpy itself. |
| `adb` (platform-tools) | Pairing, TLS connect, mDNS auto-connect, server on 5037 | **Not used at runtime.** We speak the ADB wire protocol ourselves. |
| `adb_client` crate (MIT, 3.2.3) | Rust ADB: server client, direct TCP with STLS/TLS, USB, mDNS. Blocking, one stream at a time, no pairing. Pulls in image, regex, chrono. | Reference for message framing and STLS. Not a dependency: we need async, multiplexed long-lived streams, and pairing, which it lacks [verified in source]. |
| droidsock (Apache-2.0, JS) | ADB Wi-Fi pairing, ported from AOSP | Cross-check for our pairing implementation. |
| earctl / sonyctl (Daan) | Rust daemon + CLI + Waybar module, bluer, axum HTTP API | Architecture base; Waybar conventions to match. |

## 3. Protocols that exist

- **ADB transport** [verified in AOSP `adb.h`, `adb_client`]: 24-byte header
  (command, arg0, arg1, length, crc32, magic), messages CNXN, AUTH, STLS, OPEN,
  OKAY, WRTE, CLSE. `A_VERSION` 0x01000001 (checksum may be skipped),
  `A_STLS_VERSION` 0x01000000, `MAX_PAYLOAD` 1 MiB.
- **ADB TLS** [verified in AOSP `tls/`]: after STLS, TLS 1.3 only, mutual
  certificates. The client cert carries the host's RSA ADB key.
- **ADB pairing** [verified in AOSP `pairing_auth/`, `pairing_connection/`]:
  TLS 1.3, then BoringSSL's SPAKE2 over edwards25519 with names
  `"adb pair client"` / `"adb pair server"`. The password is the 6-digit code
  plus TLS exported keying material. Then HKDF-SHA256 with info
  `"adb pairing_auth aes-128-gcm key"`, then AES-128-GCM with a
  little-endian counter nonce per direction. PeerInfo is 8192 bytes: type
  byte (0 = RSA public key, 1 = GUID) plus data.
- **mDNS**: the phone advertises `_adb-tls-connect._tcp` (port random per
  enable) and, while the pairing dialog is open, `_adb-tls-pairing._tcp`
  [verified on the phone].

## 4. Nothing-specific discoveries

- The Glyph hardware is exposed to the Android framework as **lights**:
  `ILightsManager.getLights()` returns 22 lights, ids 102-125 plus 500
  [verified]. The shell user holds `CONTROL_DEVICE_LIGHTS` [verified]. So the
  shell agent can almost certainly open a `LightsManager` session and drive
  the Glyphs with no Nothing SDK, no API key and no APK [inferred; the write
  test still needs Daan's OK].
- The official Glyph SDK talks to `com.nothing.thirdparty/.GlyphService`,
  guarded by `com.nothing.ketchum.permission.ENABLE`, protection level
  **normal** [verified]. It is a fallback if the lights route misbehaves.
- Glyph feature toggles are plain settings keys (`led_effect_enable`,
  `led_brightness_value`, `glyph_*`, `led_effect_*_enalbe` with Nothing's
  typo) [verified]. Shell has `WRITE_SECURE_SETTINGS`, so they can be read and
  written. Whether the Nothing Settings UI reacts live to external writes is
  [open].
- Glyph Progress (`com.nothing.glyphnotification`) listens to notifications;
  its switch is `settings system glyph_progress_main_switch` (0 on this phone)
  [verified]. Nothing documents that Android 16 Live Updates feed it
  [documented].
- Reverse wireless charging state: `nt_wireless_reverse_charge`,
  `nt_reverse_charging_limiting_level` [verified].
- Earbuds: `earctl` already talks to Nothing Ear directly over RFCOMM, which is
  the better path. The phone adds nothing. Fast Pair has no PC-side use.

## 5. Android discoveries: the shell agent

This is the central finding. The ADB shell user (uid 2000) holds
`CONTROL_DEVICE_LIGHTS`, `MEDIA_CONTENT_CONTROL`, `MANAGE_NOTIFICATIONS`,
`STATUS_BAR_SERVICE`, `WRITE_SECURE_SETTINGS`, `BATTERY_STATS`,
`READ_PHONE_STATE`, `READ_CLIPBOARD_IN_BACKGROUND` and more [verified]. A
small Java program started with `app_process` inherits all of it. On the real
phone:

| Capability | Mechanism | Event-driven? | Status |
|---|---|---|---|
| Notifications | `NotificationListenerService.registerAsSystemService` | yes | **works** [verified] |
| Media | `ISessionManager` tokens + `MediaController` callbacks + sessions listener | yes | **works** [verified] |
| Battery | `BatteryManager` over `batterystats`/`batteryproperties` binders | **no**: broadcasts are impossible without a ProcessRecord [verified in AOSP `BroadcastController`] | works; needs polling |
| Lights / Glyph | `ILightsManager` | n/a (commands) | listing works; write untested |
| Clipboard | `IClipboard.addPrimaryClipChangedListener` | yes | proven by scrcpy [documented], untested here |
| Settings (DND, Glyph toggles) | `settings` provider / listener `onInterruptionFilterChanged` | yes for DND | [inferred] |

Battery polling costs one binder call per interval. Timers based on uptime
(`Handler.postDelayed`) stop while the phone is in deep sleep, so the agent
never wakes the phone to poll [documented Android behaviour]. Battery changes
slowly; one read per minute while the phone is awake is plenty.

## 6. Connection options

| Option | Verdict |
|---|---|
| USB ADB | Supported via native USB (no adb server). Zero network cost. |
| Wireless debugging (TLS, paired) | **Primary.** Encrypted, mutually authenticated, no root. |
| `adb tcpip 5555` | **Rejected.** Plaintext, LAN-exposed, actively scanned. |
| Own Android app + own protocol (KDE Connect style) | Rejected for now. The shell agent gives the same data with nothing to install and no background app on the phone. |
| Bluetooth | No need: nothing we want is only reachable over BT. |

## 7. Discovery and reconnection

- Wireless debugging uses a random port per enable, so we discover it with
  mDNS `_adb-tls-connect._tcp` and match the service instance (`adb-<serial>-…`)
  against the paired phone [verified format].
- Before pairing, the phone advertised nothing; after pairing it advertised at
  once [verified]. Why is [open].
- Whether wireless debugging survives a Wi-Fi drop or reboot on Nothing OS 4.1
  (Android 16) is [open]. AOSP disables it on network change unless the
  network is trusted [documented]. **Needs a test with Daan.**
- Android 17's "ADB Wi-Fi 2.0" will fix auto-reconnect on the phone side;
  it does not apply here [documented].
- Linux side: suspend/resume via logind `PrepareForSleep`; mDNS announcements
  trigger reconnects; exponential backoff otherwise.

## 8. Battery and resource implications

- The phone runs one idle `app_process` JVM (~20-40 MB RSS [inferred from
  scrcpy], to be measured) that sleeps in its Looper and only wakes for real
  events. No wakelocks, no alarms.
- One TLS TCP connection. We use TCP keepalive with a long idle time instead
  of app-level pings every few seconds.
- The agent is started detached, so a Wi-Fi blip does not kill it. It exits by
  itself after a period with no client, so an abandoned agent can't linger.
- The laptop runs one daemon. Waybar reads a push stream instead of spawning
  curl + jq every 15 s the way the earctl/sonyctl scripts do.

## 9. Security implications

- Pairing binds our own RSA key into the phone's ADB trust store. The key
  lives in `~/.local/share/phonectl/adbkey` with mode 0600, the same model as
  `~/.android/adbkey` and ssh keys. Secret Service would add a desktop-session
  dependency for little gain; revisit if Daan wants it.
- An ADB shell is powerful. The agent exposes only a fixed set of typed
  operations; the daemon never forwards arbitrary shell commands from IPC
  clients.
- Local IPC is a Unix socket in `$XDG_RUNTIME_DIR` (mode 0700 directory). That
  beats earctl's HTTP port on 127.0.0.1, which every local user and every
  browser tab can reach.
- The mDNS service name leaks the phone serial on the LAN. Android already
  does this; we add nothing.

## 10. Licensing

- Our code: MIT (like lumend). No KDE Connect or scrcpy code is copied.
  scrcpy's `app_process` context workaround is a technique; if any of its code
  is adapted into the agent, we keep its Apache-2.0 notice on that file.
- Pairing is implemented from AOSP (Apache-2.0) and BoringSSL (ISC-style)
  specifications. We cross-check against droidsock (Apache-2.0) without
  copying it.

## 11. Gap analysis

| Need | Existing solution | Gap |
|---|---|---|
| Phone state/events on Linux without an app | none; KDE Connect needs its app | **build: shell agent + Rust daemon** |
| ADB in Rust, async, with pairing | adb_client (sync, no pairing) | **build: own transport + pairing** |
| Glyph from a PC | none without root | **build: lights via shell agent** |
| Waybar phone module, push-based | none | **build** |
| Screen mirroring | scrcpy | reuse (optional launcher only) |

## 12. Proposed feature set (v1)

Generic Android: connection state, device info, battery (+ charging, current,
temperature), notifications (list, live events, dismiss), media (now playing,
play/pause/next/prev, live events), DND mode, clipboard (phone to Linux and
back), screenshot, find-phone (ring), Wi-Fi/cellular info.

Nothing: Glyph (light test, patterns, brightness), Glyph feature toggles,
Glyph Progress switch, reverse charging state.

Not in v1: SMS/calls/contacts (needs content providers plus a UX of its own),
file transfer (MTP/`sync` later), screen casting (scrcpy exists).

## 13. Explicitly not reinvented

scrcpy (screen), MTP stacks, KDE Connect's protocol, Nothing's Glyph SDK
(unless lights fail), Bluetooth earbud control (earctl).

## 14. Unknowns that need the phone

1. Glyph write through `LightsManager` (turns lights on; needs Daan's OK).
2. Wireless debugging after Wi-Fi off/on, network change, reboot.
3. Agent memory and battery cost over a day (`dumpsys meminfo`,
   `batterystats`).
4. Whether adbd accepts our native connection while the stock adb server is
   also connected (dev convenience).
5. Glyph light id to physical zone mapping.

## 15. Confidence on the major conclusions

| Conclusion | Confidence |
|---|---|
| Shell agent can deliver notifications + media events without an APK | high (verified) |
| Battery needs polling from the agent | high (verified in source and on device) |
| Glyph controllable via LightsManager from shell | medium (permission + listing verified, write untested) |
| Native Rust ADB TLS + pairing is feasible | high (spec verified; adb_client and droidsock prove it) |
| Fully automatic Wi-Fi reconnect without touching the phone | medium-low until tested |
