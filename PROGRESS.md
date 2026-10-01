# Progress

Working state for continuing after a context reset. Newest at the top of each
section.

## Constraints (from Daan)

- No runtime dependency on KDE Connect or any other prebuilt tool. Our own
  stack only. Rust crates are fine. KDE Connect may be used during development
  for comparison.
- "AirCTL" means `../earctl`. Use its architecture as the base and improve it.
  Match the earctl/sonyctl Waybar module conventions exactly.
- Target phone: Nothing Phone (4a) Pro (A069P / FroggerPro, Android 16), not
  the Phone 2 from the brief.
- Git: Daan is the only author. No Claude trailers. Short human commit
  messages, shown to Daan before committing. Repo name not final
  (`phonectl` is a working name).
- Subagents: Sonnet or Haiku only.

## 2026-10-01: direction change

Wireless debugging only exists on Wi-Fi (Android switches it off when Wi-Fi
drops or changes, seen twice on this phone), so the ADB + shell-agent design
cannot reach the phone on 5G or another network. Also Daan's payment app
refuses to run while debugging is on. phonectl is now an Android app + laptop
daemon over Tailscale (Bluetooth fallback); ADB is setup-only. See README.md
and docs/architecture.md. The shell-agent notes below stay as research.

Done and tested on the real devices: Tailscale link + reconnect (daemon
restart, app restart/update, Wi-Fi ↔ 5G), clipboard both ways (incl. the
automatic phone path), notifications (post, update, remove, icons), call →
pause/resume Spotify, Waybar module, phone panel, over-the-link updates.

Not yet tested: Bluetooth fallback (phone and laptop not BT-paired yet),
laptop suspend/resume and laptop network change, multi-day battery numbers.

## Phase status (ADB design, superseded)

- [x] Phase 0: repo inspection (earctl, sonyctl, lumend, waybar config)
- [x] Phase 1: research tracks (docs/research/*.md)
- [~] Phase 2: device investigation (docs/research/device-observations.md)
  - [x] identity, packages, settings, shell permissions
  - [x] shell agent spike: lights listing, battery binder, media sessions
  - [x] MediaController with system context (Spike3): works via
        ISessionManager tokens + `new MediaController(shellCtx, token)`;
        MediaSessionManager itself NPEs under app_process
  - [x] notification listener from shell: NotificationListenerService +
        hidden registerAsSystemService works (STATUS_BAR_SERVICE)
  - [ ] Glyph light write test (needs Daan's OK before touching the lights)
  - [ ] ADB auto-reconnect behaviour after Wi-Fi drop / phone reboot
- [x] Phase 3: architecture doc (`docs/architecture.md`) + report
      (`docs/research/REPORT.md`)
- [~] Phase 4: implementation
  - [x] `adb` crate: message codec, host key + Android pubkey encoding
        (cross-checked byte-for-byte against `adb pubkey`), CNXN/AUTH/STLS
        handshake, stream multiplexing with OKAY flow control, TLS 1.3 client
  - [ ] pairing (SPAKE2)
  - [ ] mDNS discovery
  - [ ] first live connection to the phone without the adb binary
  - [ ] agent (`agent/`, build.sh works; only GlyphProbe so far)
  - [ ] daemon, IPC, CLI, Waybar

Decisions taken (2026-09-14): name **phonectl**, licence MIT, Glyph write test
approved by Daan (not yet run: the phone dropped off ADB first).

## Key findings so far

- Shell uid holds CONTROL_DEVICE_LIGHTS, MEDIA_CONTENT_CONTROL,
  MANAGE_NOTIFICATIONS, STATUS_BAR_SERVICE and more, so an `app_process` agent
  (scrcpy model, no APK install) can do most of the privileged work.
- `app_process` agents cannot receive broadcasts (no ProcessRecord). Battery
  must come from BatteryManager binder getters, polled on an uptime timer that
  never wakes the phone.
- adb_client (MIT) handles TLS (STLS + rustls) but not pairing. Pairing needs
  our own SPAKE2 (BoringSSL spake25519 variant) + HKDF
  ("adb pairing_auth aes-128-gcm key") + AES-128-GCM with counter nonces.
  References: AOSP pairing_auth/, pairing_connection/, droidsock
  `src/api/pairing.mjs` (Apache-2.0).

## Scratch locations

Spikes: `$SCRATCH/spike` (Java, built with build-tools 36.1.0 aidl/d8 against
android-36). Pushed as `/data/local/tmp/phonectl-spike.dex`; delete it from
the phone when done.
