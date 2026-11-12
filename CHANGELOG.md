# Changelog

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
