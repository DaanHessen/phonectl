# ADB connectivity for phonectl — research notes

Scope: how to reconnect ADB to a Nothing Phone 2 (Android 14/15, no root) automatically
and cheaply when it joins a trusted network, without insecure shortcuts (no `tcpip 5555`),
plus the Rust crates to build the daemon on. Confidence is marked per claim:
**High** = primary source (AOSP source/docs, official blog, crates.io metadata),
**Medium** = credible secondary source (XDA, well-known forums) not independently verified against source,
**Low** = inference or unconfirmed single source. Anything not marked is High.

---

## 1. Android wireless debugging (Adb Wi-Fi) internals

### 1.1 TLS pairing and connection, mDNS service types

Primary source: AOSP `packages/modules/adb/docs/dev/adb_wifi.md`
(https://android.googlesource.com/platform/packages/modules/adb/+/HEAD/docs/dev/adb_wifi.md)
and `adb_mdns.h`/`adb_mdns.cpp`
(https://android.googlesource.com/platform/packages/modules/adb/+/bb72e3b1/adb_mdns.h).

- Three mDNS service types are advertised on the LAN (High):
  - `_adb._tcp` — legacy service, only present when `adb tcpip <port>` has been used.
  - `_adb-tls-pairing._tcp` — advertised while the device's Pairing Server (QR/6-digit
    code screen) is open in Developer Options.
  - `_adb-tls-connect._tcp` — advertised whenever the device's persistent TLS adbd
    server is active and reachable.
- Pairing uses mutual TLS: each side has an X.509 cert derived from its ADB RSA keypair
  (client: `~/.android/adbkey`/`adbkey.pub`); trust is bootstrapped by a short-lived
  shared secret (6-digit pairing code or 10-digit QR secret) used in a PAKE-style
  exchange before the certs are exchanged and pinned (High, from adb_wifi.md; the doc
  states the secret seeds mutual authentication — it does not explicitly name "SPAKE2"
  in the text captured, so treat the specific PAKE variant as **Medium** rather than
  confirmed by name; the broader mechanism — PAKE-derived trust before cert pinning —
  is High and consistent with the public AOSP security writeups on Android 11 wireless
  debugging).
- Once paired, the device's public key is added to the host-of-record and the device
  remembers the host's pubkey (same trust store as regular USB `adb_keys`); a full
  reboot generally does not require re-pairing, only re-establishing the TLS connection.
- **Port**: when "Wireless debugging" is toggled on, adbd's TLS server binds to a
  **random TCP port each time it starts** (device doc: "adbd listens on a TCP server
  socket, port picked at random"). This is unlike `adb tcpip <port>`, where the port is
  whatever you specified. Practical corollary: you cannot hardcode a port for the
  Wireless-debugging TLS server; the host must discover it via mDNS or `adb pair`
  output each time. (High)
- **What toggling "Wireless debugging" off/on or a reboot does, pre‑"ADB Wi-Fi 2.0"
  (i.e. current Android 14/15 behavior on the Nothing Phone 2)**:
  - The feature has historically **not persisted across reboot** — after a reboot the
    "Wireless debugging" switch itself is often left on in UI state but the adbd TLS
    server/mDNS advertisement has to restart and re-derive a new random port; multiple
    independent reports say the wireless ADB connection is lost on reboot and Android
    used to auto-disable the toggle after a period of network inactivity, requiring the
    user to re-enable it manually (Medium — XDA Forums:
    https://xdaforums.com/t/android-12-developer-options-adb-wireless-debugging-option-keeps-turning-off.4461375/;
    corroborated by Google's own framing of the pain point it fixes in ADB Wi-Fi 2.0,
    see §1.4 below).
  - "Always allow on this network" is the trusted-network / auto-approve toggle shown
    when a new host from that Wi-Fi first connects; once checked, subsequent
    connections from a host already known (paired) on that same network don't need a
    manual on-device confirmation dialog (Medium — XDA:
    https://xdaforums.com/t/guide-how-to-enable-adb-via-wifi.4651610/). The public
    AOSP docs available to this research did not expose the exact key (BSSID vs SSID)
    used for this network-trust record; treat "keyed by BSSID" as **unconfirmed** —
    do not build logic that depends on it being BSSID-specific.
  - There is **no supported script-based way to seed this trust state** ahead of time:
    the adbd in-memory keystore for the network trust list is written back to disk on
    daemon shutdown, overwriting anything edited by hand, so pre-seeding a BSSID/SSID
    entry only works if wireless debugging has already been approved once through the
    on-device GUI (Medium — XDA guide, same source as above).

### 1.2 Host-side auto-connect via mDNS

- The adb **server** (the `adb` background process on the Linux host, not `adbd` on the
  phone) performs mDNS discovery for all three service types on startup and whenever
  its mDNS backend is active. When it sees a `_adb-tls-connect._tcp` instance whose
  advertised device GUID matches a device the host has previously paired with, it
  attempts to auto-connect. (High — adb_wifi.md, confirmed by the "host begins mDNS
  discovery... results in a connection attempt" architecture described in the fetched
  doc.)
- `adb mdns services` — lists currently discovered mDNS services (both pairing and
  connect) that the host's `adb` server can see right now. Useful as a polling/manual
  fallback but not needed if the daemon watches mDNS itself. (High, documented adb
  subcommand)
- `ADB_MDNS_AUTO_CONNECT` — environment variable for the adb server controlling which
  service names/instance names are eligible for automatic connect-on-discovery
  (default behavior auto-connects `_adb-tls-connect._tcp` instances for already-paired
  devices). Exact default value/format was not directly quoted from a primary source in
  this research pass — treat the mechanism (it exists, and gates auto-connect
  eligibility) as **Medium** confidence, not the exact syntax.
- **mDNS backend**: as of newer platform-tools, the adb server ships two backends:
  a Bonjour/mDNSResponder-compatible backend and a self-contained "openscreen"
  backend used when Bonjour/Avahi's mDNS responder isn't available; `ADB_MDNS_OPENSCREEN=1`
  forces the openscreen backend, `=0` forces the legacy one (Medium — corroborated by
  multiple secondary sources but not read directly from the platform-tools source in
  this pass). On this machine, `avahi-daemon` is running, so the OS already speaks
  mDNS/DNS-SD — that's a strong argument for the daemon to talk to Avahi directly (via
  D-Bus) or use a self-contained mDNS-SD library, rather than spawning the `adb` binary
  and hoping its bundled mDNS backend and the system's Avahi don't fight over
  the multicast socket (see §4).
- **Important implication for phonectl**: none of this requires phonectl to
  reimplement TLS pairing. Once the phone has been paired once (by hand, via
  `adb pair` or the Android Studio flow), the ongoing job is only: (a) notice via mDNS
  that `_adb-tls-connect._tcp` is being advertised by the known device, and (b) run
  `adb connect host:port` (or the equivalent over the adb server protocol) — which is
  exactly what the stock `adb` server already does on its own if left running with
  mDNS discovery active. This significantly lowers the bar for what phonectl's own
  code needs to do; it can lean on `adb start-server` + built-in mDNS auto-connect for
  the connect step, and focus its own logic on lifecycle (start the server when the
  network changes, handle "wireless debugging currently off" recovery) and event
  plumbing (no polling).

### 1.3 Re-enabling wireless debugging from the shell; security implications

- `adb shell settings put global adb_wifi_enabled 1` is a widely used and reportedly
  working way to toggle wireless debugging back on from an existing shell —
  **but it requires you already have some form of adb shell access** (USB, or an
  already-authorized wireless session) to run it; it doesn't help you get the *first*
  connection after wireless debugging has been fully turned off and no session exists.
  (Medium — this is common community knowledge repeated across ADB cheat-sheet pages;
  not verified against AOSP source in this pass.)
- The `shell` UID (`com.android.shell`) is granted `WRITE_SECURE_SETTINGS` by the
  platform permission set by default, which is why `settings put global ...` from an
  adb shell works without any extra `pm grant` step — that grant step is only needed
  for a *third-party app* that wants the same permission, not for the shell user
  itself. (Medium — consistent with widely repeated ADB documentation, not read
  directly from AOSP's shell permission manifest in this pass.)
- **Security implication of using this path**: turning `adb_wifi_enabled` on via a
  shell you already have is not materially riskier than the user turning the toggle on
  from Settings — it doesn't skip pairing/TLS, and it doesn't create a new attack
  surface beyond what wireless debugging already has when the user has manually
  enabled it. The risk is scoping *when* phonectl does this: doing it automatically
  and silently means a compromised or physically-accessed host session could
  re-enable wireless debugging on the phone without the user pressing anything, so
  phonectl should still require the *first* enable/pairing to be a deliberate
  user action, and should not attempt to auto re-enable if the user has explicitly
  turned wireless debugging off in Settings (an explicit off is likely intended to be
  sticky).

### 1.4 ADB Wi-Fi 2.0 (Android 17 + platform-tools 37, announced Sept 2026) — does NOT apply to this phone

This is highly relevant and time-sensitive: Google published "Introducing Fast and
Reliable Wireless Debugging with ADB Wi-Fi 2.0"
(https://android-developers.googleblog.com/2026/09/wireless-debugging-adb-wifi-2.html,
also covered by 9to5Google: https://9to5google.com/2026/09/10/google-details-adb-wi-fi-2-0-for-more-reliable-wireless-android-debugging/
and Android Authority: https://www.androidauthority.com/android-wireless-adb-auto-reconnect-3624945/)
on 2026-09-10, i.e. days before this research.

- It replaces the adb server's mDNS stack ("replaced both Bonjour and legacy mDNS")
  specifically to survive network changes/reboots better, and changes adbd so it
  **auto-disables wireless debugging on untrusted networks and re-enables itself on a
  trusted one** — i.e. genuine trusted-network-aware auto-reconnect, no user toggling.
  (High, from the announcement.)
- Reported gains: 32% better auto-connect success rate, 66% faster connections for 90%
  of connections. (High, as claimed by Google's post.)
- **Minimum requirements stated by Google: Android 17, Android SDK Platform-Tools
  37.0.0, Android Studio "Quail" or later.** (High)
- **Consequence for this project**: the Nothing Phone 2 ships Android 14/15 and there is
  no indication (nor plausible expectation) that Nothing will backport Android 17's
  ADB Wi-Fi 2.0 stack to it. phonectl must therefore be designed against the
  **legacy** (pre-2.0) behavior described in §1.1–1.3: random port per enable, fragile
  persistence across reboot/network change, no built-in trusted-network auto-reconnect
  on the device side. All of the "daemon does the reconnect work" design pressure in
  the task brief is justified — this is precisely the gap ADB Wi-Fi 2.0 closes for newer
  Android, and precisely the gap this phone will keep having.

### 1.5 Legacy `adb tcpip 5555`

- Not persistent: reverts on reboot; the "fixed" port some guides set via
  `service.adb.tcp.port` in `build.prop`/`default.prop` requires root/write access to
  system partitions, which is out of scope for this no-root project. (Medium — XDA:
  https://xdaforums.com/t/how-to-make-adb-listen-to-tcpip-5555-after-reboot.1825359/)
- Security: this mode has **no TLS and no pairing** — any host that can reach port 5555
  gets an unauthenticated-at-the-transport-layer adb connection (the original RSA
  key-based authorization dialog still applies for *new* unknown keys, but the socket
  itself is open to the whole LAN/any reachable network, and mass scanners like
  Shodan actively look for open 5555). This is the documented reason the task brief
  rules it out; it's a good decision. (High-confidence risk characterization; e.g.
  HackTricks: https://hacktricks.wiki/en/network-services-pentesting/5555-android-debug-bridge.html)

---

## 2. USB detection

### 2.1 Nothing Phone 2 USB vendor ID

**Not confirmed in this research pass.** I checked:
- The community-maintained `51-android.rules` udev database
  (M0Rf30/android-udev-rules, mirrored across several forks) — no "Nothing" entry
  found by direct search of the current file.
- General web search for Nothing Phone lsusb/idVendor reports — no reliable primary
  hit.

Recommendation (Low confidence, action item not a fact): don't hardcode a vendor ID
you haven't verified against the actual device. Have the daemon read the ID by running
`lsusb` or inspecting `/sys/bus/usb/devices/*/idVendor` while the phone is connected via
USB once, and store the observed VID (and PID) in phonectl's own config, or — better —
avoid needing a fixed VID at all by using the adb server's own
`host:track-devices-l` event stream (see §2.2 and §3), which reports devices adb already
recognizes regardless of VID, so phonectl never needs to special-case Nothing's USB
IDs at all for the "USB present" signal.

### 2.2 Event-driven device detection without polling

- The adb server protocol has a `host:track-devices` service and an `-l` variant,
  `host:track-devices-l`, that keeps the client's TCP socket open and pushes a new
  4-hex-digit-length-prefixed device list every time a device is added/removed or
  changes state (e.g., `offline` → `device`, or `unauthorized` → `device`). This is
  exactly the "no polling" primitive requested. (High — AOSP `docs/dev/services.md`:
  https://android.googlesource.com/platform/packages/modules/adb/+/HEAD/docs/dev/services.md;
  corroborated by a plain-English writeup: https://kellansu.com/posts/adb-protocol/)
- Practical design: phonectl's daemon opens one persistent TCP connection to
  `127.0.0.1:5037` (the adb server, started via `adb start-server` if not already
  running), sends `host:track-devices-l`, and reads length-prefixed updates forever.
  Both USB arrival and wireless TLS connect/disconnect show up as device-state
  transitions on this same stream — one mechanism covers both wired and wireless
  presence detection, which simplifies the daemon considerably.

---

## 3. Talking to the adb server protocol from Rust vs. spawning `adb`

The adb *host* protocol (talking to `adbd` on port 5037, i.e. `host:*` services,
`host:transport:<serial>` followed by device-local services like `shell,v2:...`,
`sync:`, `exec:screencap`) is a simple length-prefixed ASCII-header protocol over TCP,
documented in `docs/dev/services.md` and the still-referenced legacy `SERVICES.TXT`.
Two viable approaches:

1. **Spawn the `adb` binary** for one-shot operations (`adb -s <serial> shell ...`,
   `adb connect host:port`) and only implement `host:track-devices-l` directly for the
   event stream. Lowest engineering risk, but couples the daemon to the platform-tools
   `adb` binary being installed and its process-spawn overhead per call.
2. **Talk to 127.0.0.1:5037 directly** for everything (track-devices, transport
   selection, shell v2, sync, exec) — avoids process spawn overhead and gives the daemon
   full control (e.g. can multiplex, no dependence on `adb`'s CLI parsing quirks) but
   means implementing/depending on a chunk of the protocol.

### Rust crates evaluated

- **`adb_client`** (crates.io, https://crates.io/crates/adb_client, repo
  https://github.com/cocool97/adb_client). Latest stable: **3.2.3**, last published
  2026-08-02. MIT license. Implements both the ADB *host* protocol and (per its own
  README) both server-mediated and USB-direct device communication, has built-in mDNS
  device discovery, and a companion `adb_cli` binary. MSRV 1.88. Over 1.1M cumulative
  downloads (across all versions) per crates.io — actively used, not a toy. This is the
  strongest general-purpose candidate for phonectl: it removes the need to
  reimplement `host:track-devices-l` parsing, shell v2, sync, and exec:screencap by
  hand, and its own mDNS discovery feature overlaps usefully with §1.2. Verify at
  integration time whether its async support meets your tokio-based daemon (check
  current README for sync-vs-async API shape before committing).
- **`forensic-adb`** (crates.io, https://crates.io/crates/forensic-adb, repo
  https://github.com/kpcyrd/forensic-adb, docs https://docs.rs/forensic-adb). Latest
  stable: **0.8.1**, published 2025-03-28 (i.e., no update in ~18 months as of this
  research — noticeably less active than `adb_client`). MPL-2.0. Explicitly Tokio-based
  (async), forked from Mozilla's `mozdevice` (used in Firefox for Android test
  infrastructure) with root-detection commands intentionally removed so it never runs
  privileged commands on the device by default — a good safety property for a project
  that must stay no-root. Smaller (~1.6k LOC) and more narrowly scoped than
  `adb_client`. Worth a direct dependency-weight/API comparison against `adb_client`
  before deciding; `forensic-adb`'s Tokio-native design may integrate more cleanly if
  the daemon is already tokio-based, at the cost of a less actively maintained
  upstream.
- No other actively maintained pure-Rust host-protocol crate surfaced in this research
  as a clearly better third option; the field is effectively `adb_client` vs
  `forensic-adb` vs "spawn `adb`."

**Recommendation**: start with `adb_client` (0.x→3.2.3, MIT, more actively maintained,
built-in mDNS) for both the track-devices event stream and command execution
(dumpsys, screencap, Glyph-related settings pokes), falling back to spawning the real
`adb`/`scrcpy` binaries only for scrcpy launch itself (scrcpy already expects to shell
out to a platform-tools `adb`, and reimplementing scrcpy's video-forwarding protocol is
out of scope).

---

## 4. Supporting Rust crates

| Need | Recommendation | Version (crates.io, as of 2026-09-14) | Notes |
|---|---|---|---|
| mDNS discovery | `mdns-sd` | **0.21.3** (updated 2026-09-08) | Pure-safe-Rust DNS-SD, no async runtime coupling (runs its own thread, exposes results via `flume` channels usable from sync or async code), tested against Avahi on Linux specifically. Good fit since it doesn't fight over the mDNS socket the way spawning a second full mDNS responder might, and doesn't require D-Bus. Alternative: talk to the already-running `avahi-daemon` over D-Bus (via zbus) if you want to avoid a second multicast listener entirely and piggyback on the system's existing mDNS infra — more "correct" on a machine that's already running Avahi, marginally more D-Bus plumbing to write. Given `adb_client` may already bundle its own mDNS discovery (§3), first check whether relying on `adb_client`'s mDNS + `host:track-devices-l` alone is sufficient before adding a second mDNS dependency. |
| D-Bus | `zbus` | **5.19.0** (updated 2026-08-09) | Runtime-agnostic; enable the `tokio` feature (`zbus = { version = "5", default-features = false, features = ["tokio"] }`) to avoid zbus spinning up its own background executor thread when the daemon is already tokio-based. |
| logind sleep/resume signal | `zbus` directly against `org.freedesktop.login1.Manager`'s `PrepareForSleep` signal, or the `zbus_systemd::login1` generated bindings (docs.rs/zbus_systemd) | — | `PrepareForSleep(true)` fires before suspend, `(false)` after resume — useful trigger to re-check ADB connectivity after resume without polling. |
| NetworkManager signals | Hand-rolled zbus proxy against `org.freedesktop.NetworkManager`, or `nmrs` (https://github.com/freedesktop-rs/nmrs) | — | Watch `StateChanged`/`PropertiesChanged` on the active connection to detect "joined a network" transitions and the SSID/BSSID, to gate the reconnect attempt to trusted networks. `nmrs` is a newer, purpose-built async wrapper; a hand-written zbus proxy generated via `zbus-xmlgen` against the live `org.freedesktop.NetworkManager` interface is also reasonable and keeps the dependency footprint minimal. |
| Secret storage (adb key material lives in `~/.android/adbkey` already, unencrypted by adb itself — no crate needed for that) | If phonectl needs its own secrets (e.g. a paired-host token), `oo7` | **0.6.0** (updated 2026-07-17) | Modern async Secret Service / portal client; picks the right backend automatically depending on sandboxing. Preferred over `secret-service` for new code on the freedesktop Secret Service. |
| | `secret-service` (fallback/alternative) | **5.2.0** (updated 2026-08-29) | Contrary to older commentary suggesting this crate was stale, crates.io shows an active release as recently as 2026-08-29 — re-verify its async story before ruling it out; `oo7` remains the more modern choice but `secret-service` is not dead. |
| Config | `toml` + `serde`/`serde_derive` | toml **1.1.6+spec-1.1.0** (updated 2026-09-10) | Standard, low-risk choice; no reason to deviate. |
| Async runtime | `tokio` | **1.53.1** | De facto standard; `adb_client`/`forensic-adb`/`zbus` all have tokio integration paths as noted above. |
| Tray icon | `ksni` | **0.3.6** (updated 2026-07-15) | Actively maintained (iovxw), implements the KDE/freedesktop StatusNotifierItem spec directly (D-Bus based, so it composes naturally with a zbus-based daemon), Unlicense, ~87k downloads/month. Good fit for a Waybar/KDE-adjacent tray icon. |
| IPC (daemon ↔ CLI/Waybar) | Unix socket + JSON lines (e.g. `tokio::net::UnixListener` + `serde_json` line-delimited) vs a D-Bus service | — | A D-Bus service is more idiomatic on this stack (you're already pulling in zbus for logind/NetworkManager) and gives you introspection, PolicyKit-style access control, and signals for free — arguably the better choice here specifically *because* zbus is already a dependency. A raw Unix-socket JSON-lines protocol is simpler to hand-roll and easier to `nc`/`jq` for debugging, at the cost of reinventing what D-Bus already gives you. Given the daemon already needs a zbus connection for PrepareForSleep and NetworkManager signals, exposing its own service on the session bus is the lower-total-complexity option. |

---

## 5. Security

- **ADB key storage**: the host's ADB identity is `~/.android/adbkey` (private key,
  unencrypted on disk, standard `adb` behavior) and `~/.android/adbkey.pub`. This is
  Google's own long-standing design — the private key file's confidentiality relies
  entirely on filesystem permissions (mode 600, owned by the invoking user); phonectl
  should not weaken this (e.g. don't relax permissions, don't copy the key into a
  world-readable location) and does not need to build its own key-encryption layer for
  this file, since that's not how upstream `adb` protects it either. If phonectl adds
  its own secret (e.g., a shared token with the phone-side automation, if any), that's
  where `oo7`/secret-service is worth using — not for adbkey itself.
- **LAN threat model of wireless debugging**: even fully paired, the wireless-debugging
  TLS connection accepts commands from any host holding a private key the phone has
  authorized — i.e., the security boundary is "is this host's key on my authorized
  list," not "is this host on my LAN." The main incremental risk versus USB debugging is
  exposure: a TLS-wrapped, authenticated adb port is still a bigger network attack
  surface than a USB-only setup, though meaningfully smaller than plain `tcpip 5555`
  because of the mutual-TLS/pairing requirement. The likeliest realistic risk for this
  project isn't remote LAN attackers (mutual TLS defeats casual scanning) but a
  same-network device that has *also* somehow acquired an authorized keypair, or a
  compromised host machine using its already-authorized key to reach the phone — the
  same trust model as SSH `authorized_keys`.
- **Why not `tcpip 5555`**: covered in §1.5 — no TLS/pairing at the transport layer,
  open to anything that can route to the port, actively scanned for on the public
  internet by tools like Shodan when phones end up behind port-forwarding/public IPs by
  misconfiguration. The task brief's decision to avoid it is well supported.

---

## 6. Power implications on the phone (evidence, not speculation)

- I could not find primary-source (AOSP or Google) quantified figures for the specific
  combination of "wireless debugging enabled + one idle TLS adb connection held open."
  What is documented:
  - Doze/App Standby (https://developer.android.com/training/monitoring-device-state/doze-standby)
    describes the general policy: idle, stationary, unplugged devices enter Doze and
    restrict background network access and wakelocks for apps, with periodic
    maintenance windows. adbd itself is a system daemon, not a regular app subject to
    the same standby buckets, so Doze's app-level restrictions are not a reliable proxy
    for adbd's own behavior.
  - Community reports (Medium confidence, anecdotal, not measured) describe wireless
    debugging silently turning itself off after a period of network inactivity — this
    reads as Android's own pre-2.0 mitigation against exactly the "leave an idle TLS
    server listening forever" cost profile you're asking about, rather than proof of
    high cost per se (https://github.com/Genymobile/scrcpy/issues/6605).
  - No evidence found (primary or secondary) that a merely-open, idle adbd TLS socket
    holds a *partial wakelock* by itself; the more plausible mechanism for any battery
    cost is Wi-Fi radio staying associated/awake to keep the TCP connection alive
    (which is a property of the Wi-Fi power-save state, not something specific to adb),
    plus whatever periodic keepalive traffic TCP/TLS produces.
  - **Bottom line, stated honestly**: the actual idle power cost of "wireless debugging
    on + one open adb connection" was not quantified in any primary source found in
    this pass. Treat any specific percentage/mAh claim from either direction as
    unverified. If this matters for the daemon's design (e.g. deciding whether to
    proactively disconnect when the phone is expected to be idle for a long time), the
    only trustworthy path is to measure it directly on the actual device with
    `adb shell dumpsys batterystats` before/after, rather than relying on secondary
    sources.

---

## Sources

- AOSP ADB Wi-Fi architecture: https://android.googlesource.com/platform/packages/modules/adb/+/HEAD/docs/dev/adb_wifi.md
- AOSP `adb_mdns.h`: https://android.googlesource.com/platform/packages/modules/adb/+/bb72e3b1/adb_mdns.h
- AOSP adb services protocol doc: https://android.googlesource.com/platform/packages/modules/adb/+/HEAD/docs/dev/services.md
- ADB protocol writeup (secondary, protocol structure confirmation): https://kellansu.com/posts/adb-protocol/
- Google, "Introducing Fast and Reliable Wireless Debugging with ADB Wi-Fi 2.0" (2026-09-10): https://android-developers.googleblog.com/2026/09/wireless-debugging-adb-wifi-2.html
- 9to5Google coverage of ADB Wi-Fi 2.0: https://9to5google.com/2026/09/10/google-details-adb-wi-fi-2-0-for-more-reliable-wireless-android-debugging/
- Android Authority coverage of ADB Wi-Fi 2.0 / auto-reconnect: https://www.androidauthority.com/android-wireless-adb-auto-reconnect-3624945/
- XDA Forums, wireless debugging toggling off / trusted-network confirmation UX (secondary, Medium confidence): https://xdaforums.com/t/android-12-developer-options-adb-wireless-debugging-option-keeps-turning-off.4461375/ and https://xdaforums.com/t/guide-how-to-enable-adb-via-wifi.4651610/
- XDA Forums, `tcpip 5555` reboot persistence: https://xdaforums.com/t/how-to-make-adb-listen-to-tcpip-5555-after-reboot.1825359/
- HackTricks, ADB port 5555 threat model (secondary): https://hacktricks.wiki/en/network-services-pentesting/5555-android-debug-bridge.html
- `adb_client` crate: https://crates.io/crates/adb_client, https://github.com/cocool97/adb_client
- `forensic-adb` crate: https://crates.io/crates/forensic-adb, https://github.com/kpcyrd/forensic-adb
- `mdns-sd` crate: https://crates.io/crates/mdns-sd, https://github.com/keepsimple1/mdns-sd
- `zbus` crate: https://crates.io/crates/zbus, https://github.com/z-galaxy/zbus/
- `zbus_systemd` (login1/PrepareForSleep bindings): https://docs.rs/zbus_systemd/latest/zbus_systemd/login1/
- `nmrs` (NetworkManager over zbus): https://github.com/freedesktop-rs/nmrs
- `oo7` crate: https://crates.io/crates/oo7, https://github.com/linux-credentials/oo7
- `secret-service` crate: https://crates.io/crates/secret-service
- `ksni` crate: https://crates.io/crates/ksni, https://github.com/iovxw/ksni
- Android Doze/App Standby docs: https://developer.android.com/training/monitoring-device-state/doze-standby
- scrcpy issue on wireless debugging auto-disable behavior (secondary, anecdotal): https://github.com/Genymobile/scrcpy/issues/6605
- M0Rf30 android-udev-rules (checked for Nothing entry, not found): https://github.com/M0Rf30/android-udev-rules
