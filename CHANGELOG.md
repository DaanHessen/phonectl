# Changelog

## 0.5.0 (2026-11-12)

- `phonectl hotspot on|off` and hotspot state in the status (the bar's phone
  panel shows a hotspot row). Shell `TETHER_PRIVILEGED` borrowed only for
  the switch; the hotspot's saved settings are never touched.
- Do Not Disturb follows between the laptop (mako mode) and the phone, both
  ways; `phonectl dnd on|off`; `dnd_sync = false` turns it off.
- Confirmation on the phone: on a locked phone, switching the hotspot on
  waits for Allow in a notification (60 s, once). Calls from AI agents on the
  laptop always wait for that tap.
- Fix: the laptop could act on any notification key, including phonectl's
  own; now only notifications it was shown, never phonectl's own.
- Agents cannot push app builds with `phonectl update`.
- `phonectl type` works with any keyboard: key events through the shell
  identity (`INJECT_EVENTS` borrowed per call), unlocked phone only, text
  and a fixed set of keys only.
- `phonectl ring` rings at full alarm volume even on silent and stops on any
  touch of the phone (stop screen over the lock screen), after 60 s, or with
  `--stop`.
- Links both ways: `phonectl share URL` shows a link notification on the
  phone (opens on a tap); sharing a link to "Send to laptop" shows it on the
  laptop with an Open button. http/https only, never opened by itself.

## 0.4.0 (2026-11-12)

- Silent updates of the owner's own apps (vector_phone, crew, krentenweger,
  phonectl Keyboard, phonectl) from the phone store: per-package pinned
  signing keys, already installed apps only, newer versions only, shell
  `INSTALL_PACKAGES` borrowed only for the install. Switch in the app;
  `phonectl update-apps` asks for a check now.
- Command gate: every laptop call is whitelisted, rate-limited, refused
  while locked when harmful, and listed under "Recent laptop actions" in the
  app.
- `phonectl update` APKs are checked against the pinned key and version
  before Android's prompt.
- Plain HTTP only to the phone store's address; HTTPS everywhere else.

## 0.3.0 (2026-11-11)

- Automatic phone → laptop clipboard with any keyboard: a shell-identity
  instrumentation watcher, started by the laptop over a few seconds of
  wireless debugging after reboots and app updates.
- `phonectl setup-clipboard`; pairing only after a confirm on the phone;
  exported components test; docs/security.md and docs/how-it-works.md.
