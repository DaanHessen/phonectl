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

## Phase status

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
- [ ] Phase 3: architecture doc + synthesis report
- [ ] Phase 4+: implementation

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
