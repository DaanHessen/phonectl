<a id="readme-top"></a>

<div align="center">

  <img src="assets/logo.svg" alt="logo" width="200" height="auto" />
  <h1>phonectl</h1>
  
  <p>
    Your Android phone, wired into your Linux desktop. Clipboard both ways, phone notifications in your notification daemon, media that pauses for calls, and phone status in Waybar. Over Tailscale, so it works on any network.
  </p>
  
  
<!-- Badges -->
<p>
  <a href="https://github.com/DaanHessen/phonectl/graphs/contributors">
    <img src="https://img.shields.io/github/contributors/DaanHessen/phonectl" alt="contributors" />
  </a>
  <a href="">
    <img src="https://img.shields.io/github/last-commit/DaanHessen/phonectl" alt="last update" />
  </a>
  <a href="https://github.com/DaanHessen/phonectl/network/members">
    <img src="https://img.shields.io/github/forks/DaanHessen/phonectl" alt="forks" />
  </a>
  <a href="https://github.com/DaanHessen/phonectl/stargazers">
    <img src="https://img.shields.io/github/stars/DaanHessen/phonectl" alt="stars" />
  </a>
  <a href="https://github.com/DaanHessen/phonectl/issues/">
    <img src="https://img.shields.io/github/issues/DaanHessen/phonectl" alt="open issues" />
  </a>
  <a href="https://github.com/DaanHessen/phonectl/blob/master/LICENSE">
    <img src="https://img.shields.io/github/license/DaanHessen/phonectl.svg" alt="license" />
  </a>
</p>
   
<h4>
    <a href="docs/">Documentation</a>
  <span> · </span>
    <a href="https://github.com/DaanHessen/phonectl/issues/">Report Bug</a>
  <span> · </span>
    <a href="https://github.com/DaanHessen/phonectl/issues/">Request Feature</a>
  </h4>
</div>

<br />

<!-- Table of Contents -->
# :notebook_with_decorative_cover: Table of Contents

- [About the Project](#star2-about-the-project)
  * [Screenshots](#camera-screenshots)
  * [Tech Stack](#space_invader-tech-stack)
  * [Features](#dart-features)
  * [Environment Variables](#key-environment-variables)
- [Getting Started](#toolbox-getting-started)
  * [Prerequisites](#bangbang-prerequisites)
  * [Installation](#gear-installation)
  * [Running Tests](#test_tube-running-tests)
  * [Run Locally](#running-run-locally)
  * [Deployment](#triangular_flag_on_post-deployment)
- [Usage](#eyes-usage)
- [Roadmap](#compass-roadmap)
- [Contributing](#wave-contributing)
- [FAQ](#grey_question-faq)
- [License](#warning-license)
- [Contact](#handshake-contact)
- [Acknowledgements](#gem-acknowledgements)

  

<!-- About the Project -->
## :star2: About the Project

phonectl is a personal, much leaner take on KDE Connect, built for one phone
(a Nothing Phone (4a) Pro on Android 16) and one desktop (Omarchy: Hyprland,
Waybar, mako). It is a small Android app plus a Rust daemon on the laptop.

The two talk over the [Tailscale](https://tailscale.com/) network both devices
are already on, so it does not matter whether the phone is on 5G and the
laptop on school Wi-Fi: if Tailscale can reach the phone, so can phonectl.
When there is no network at all it falls back to Bluetooth.

It is built to be left running: no polling, no foreground service, no
permanent notification on the phone, and no ADB once it is set up (some
banking and payment apps refuse to run while debugging is enabled).


<!-- Screenshots -->
### :camera: Screenshots

<div align="center"> 
  <img src="assets/waybar.png" alt="Waybar module: phone icon with battery percentage" />
  <br /><br />
  <img src="assets/panel.png" alt="Drop-down phone panel: battery, Wi-Fi and mobile signal, find my phone, ringer, send clipboard" />
</div>


<!-- TechStack -->
### :space_invader: Tech Stack

<details>
  <summary>Phone</summary>
  <ul>
    <li><a href="https://kotlinlang.org/">Kotlin</a>, plain Android framework APIs (no AndroidX), ~110 KB APK</li>
    <li><a href="https://developer.android.com/reference/android/service/notification/NotificationListenerService">NotificationListenerService</a> (also what keeps the process alive)</li>
    <li>TelephonyCallback, phone-state broadcast, MediaSession, PackageInstaller</li>
  </ul>
</details>

<details>
  <summary>Laptop</summary>
  <ul>
    <li><a href="https://www.rust-lang.org/">Rust</a> with <a href="https://tokio.rs/">Tokio</a></li>
    <li><a href="https://github.com/dbus2/zbus">zbus</a>: freedesktop notifications, MPRIS, logind</li>
    <li><a href="https://github.com/bluez/bluer">bluer</a>: BlueZ RFCOMM profile</li>
    <li><a href="https://github.com/bugaevc/wl-clipboard">wl-clipboard</a></li>
    <li><a href="https://github.com/Alexays/Waybar">Waybar</a> custom module</li>
  </ul>
</details>

<details>
<summary>Transport</summary>
  <ul>
    <li><a href="https://tailscale.com/">Tailscale</a> (WireGuard) TCP, primary</li>
    <li>Bluetooth RFCOMM, fallback</li>
    <li>Newline-delimited JSON, HMAC-SHA256 mutual authentication</li>
  </ul>
</details>

<!-- Features -->
### :dart: Features

- **Clipboard sync, both ways**, with loop prevention (content hashes) and
  "newest wins" after a reconnect. Password-manager clips are never sent.
- **Notification mirroring** to mako/any freedesktop notification daemon:
  app name and icon, updates in place, removal on the phone closes it on the
  laptop, actions and "open on phone", dismissing on the laptop dismisses on
  the phone.
- **Calls pause laptop media** (any MPRIS player) and resume exactly what was
  paused when the call ends, and only if it is still paused.
- **Waybar module**: battery, charging, connection and transport; tooltip with
  Wi-Fi/mobile signal, network type (5G/LTE), carrier and call state.
- **Phone panel** (drop-down): now playing on the phone with controls, find my
  phone, ringer mode, send clipboard.
- **Survives real life**: Wi-Fi ↔ 5G, laptop suspend, daemon or app restarts,
  Tailscale reconnects. Reconnection is event-driven with backoff.
- **Updates over the link**: `phonectl update`, no ADB.

<!-- Env Variables -->
### :key: Environment Variables

None are required. Optional:

`PHONECTL_LOG`: log filter for the daemon, e.g. `info` (default) or `phonectl=debug`

Optional config lives in `~/.config/phonectl/config.toml`:

```toml
name = "my-laptop"        # how the phone shows this laptop (default: hostname)
port = 47201              # TCP port on the Tailscale address
dismiss_on_phone = true   # dismissing here also clears it on the phone
```

<!-- Getting Started -->
## 	:toolbox: Getting Started

<!-- Prerequisites -->
### :bangbang: Prerequisites

- Linux with a Wayland compositor, `wl-clipboard`, BlueZ, systemd user session
- Tailscale running on both the laptop and the phone (same tailnet)
- Rust (1.90+), JDK 17 and the Android SDK (platform 36) to build
- `adb` for the one-time setup, with Wireless debugging paired

<!-- Installation -->
### :gear: Installation

Build the phone app and the laptop binary

```bash
  git clone https://github.com/DaanHessen/phonectl.git
  cd phonectl
  (cd android && ./gradlew assembleRelease)
  cargo build --release
  ln -sf "$PWD/target/release/phonectl" ~/.local/bin/phonectl
```

Start the daemon

```bash
  cp contrib/phonectl.service ~/.config/systemd/user/
  systemctl --user enable --now phonectl
```

Set up the phone (installs the app, grants permissions, pairs). Afterwards,
turn Wireless debugging off again.

```bash
  phonectl setup
```

Add the Waybar module

```jsonc
  "custom/phonectl": {
    "exec": "phonectl waybar",
    "return-type": "json",
    "restart-interval": 10,
    "on-click-right": "phonectl clip",
    "on-click-middle": "phonectl connect"
  }
```
   
<!-- Running Tests -->
### :test_tube: Running Tests

To run tests, run the following commands

```bash
  cargo test
  (cd android && ./gradlew testDebugUnitTest)
```

<!-- Run Locally -->
### :running: Run Locally

Run the daemon in the foreground with verbose logs

```bash
  PHONECTL_LOG=debug phonectl daemon
```

Watch what happens on the link

```bash
  phonectl events
```

Read the phone app's own log (no ADB needed; states and package names only)

```bash
  phonectl diag
```


<!-- Deployment -->
### :triangular_flag_on_post: Deployment

Ship a new phone app build over the link (the first update asks for
confirmation on the phone, later ones install silently)

```bash
  (cd android && ./gradlew assembleRelease)
  phonectl update
```


<!-- Usage -->
## :eyes: Usage

Once running there is nothing to do: copy on one device, paste on the other;
notifications and calls just show up. The CLI covers the rest.

```
phonectl status [--json]      phone and link status
phonectl clip                 send the laptop clipboard to the phone now
phonectl connect              ask the phone to connect now
phonectl ring [--stop]        find my phone (rings even on silent)
phonectl ringer MODE          normal | vibrate | silent
phonectl media ACTION         toggle | play | pause | next | previous
phonectl test-notification    post a test notification on the phone
phonectl events | diag        live link events | phone app log
phonectl update | setup       app update over the link | one-time setup
```

How it connects:

1. **Tailscale TCP.** The laptop listens only on its Tailscale address. The
   phone always dials; Tailscale handles NAT and network changes, so a session
   usually survives a Wi-Fi ↔ 5G switch untouched.
2. **Bluetooth RFCOMM** when the phone has no network, or Tailscale fails
   twice. The phone moves back to Tailscale on the next network change (or
   within 10 minutes) and drops Bluetooth.
3. **Waking up.** The laptop sends a small authenticated UDP "poke" on
   startup, resume and network change, so the phone connects at once instead
   of waiting for its backoff (2 s doubling to 10 min, on a clock that stops
   while the phone sleeps). Before suspend the laptop tells the phone, which
   then stays quiet until poked.

<!-- Roadmap -->
## :compass: Roadmap

* [x] Tailscale link with event-driven reconnect
* [x] Clipboard sync both ways
* [x] Notification mirroring with actions and removal
* [x] Pause/resume media for calls
* [x] Waybar module and phone panel
* [x] Updates over the link
* [ ] Bluetooth fallback tested end to end
* [ ] Reply to messages from the laptop
* [ ] Re-check everything on Android 17


<!-- Contributing -->
## :wave: Contributing

<a href="https://github.com/DaanHessen/phonectl/graphs/contributors">
  <img src="https://contrib.rocks/image?repo=DaanHessen/phonectl" />
</a>


This is a personal project, tuned for one phone and one desktop setup. Issues
and ideas are welcome; generalising it is not a goal yet.


<!-- FAQ -->
## :grey_question: FAQ

- Why does automatic phone → laptop clipboard need a tap after a reboot?

  + Android 10+ only lets the focused app read the clipboard. phonectl notices
    clipboard changes through the system log (READ_LOGS, granted once) and
    briefly focuses an invisible activity to read the new clip, the same trick
    KDE Connect uses. Android 13+ asks for log-access consent, but only shows
    that prompt to an app on screen, so after a reboot or app update the app
    posts a quiet notification: tap it and allow. Until then, share text to
    "Send to laptop" or use the Quick Settings tile.

- Why not just ADB over Wi-Fi?

  + Android's wireless debugging only exists on Wi-Fi and switches itself off
    when the network changes, so it can never reach a phone on 5G. And some
    payment apps refuse to run while debugging is on.

- Why not Wi-Fi Direct for offline use?

  + Android asks the user to accept every Wi-Fi Direct connection, and the
    Linux side (iwd) has no usable group-owner support. Bluetooth is simpler
    and good enough for the small amount of data involved.

- Can I reply to a message from the laptop?

  + Not yet: mako has no text input. Other notification actions work.


<!-- License -->
## :warning: License

Distributed under the MIT License. See LICENSE for more information.


<!-- Contact -->
## :handshake: Contact

Daan Hessen - [@DaanHessen](https://github.com/DaanHessen)

Project Link: [https://github.com/DaanHessen/phonectl](https://github.com/DaanHessen/phonectl)


<!-- Acknowledgments -->
## :gem: Acknowledgements

 - [KDE Connect](https://kdeconnect.kde.org/), for showing what a phone-desktop bridge can be
 - [scrcpy](https://github.com/Genymobile/scrcpy), whose research shaped the early ADB experiments
 - [Tailscale](https://tailscale.com/)
 - [Shields.io](https://shields.io/)
 - [Awesome README Template](https://github.com/Louis3797/awesome-readme-template)

<p align="right">(<a href="#readme-top">back to top</a>)</p>
