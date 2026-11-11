#!/usr/bin/env bash
# Builds the laptop daemon and installs it as a real file in ~/.local/bin.
#
# Until 2026-11 ~/.local/bin/phonectl was a symlink into target/release, so a
# `cargo clean` (done to free disk space) deleted the running service's binary
# and the phone link was down with no visible error. Now the build happens in
# a cache dir and the result is copied; cleaning the build never touches the
# installed binary.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/.cache/phonectl-build}"
cargo build --release
dest="$HOME/.local/bin/phonectl"
mkdir -p "$(dirname "$dest")"
# Replace atomically (a running daemon keeps its old inode).
install -m 755 "$CARGO_TARGET_DIR/release/phonectl" "$dest.new"
mv -f "$dest.new" "$dest"
echo "installed $dest"

unit="$HOME/.config/systemd/user/phonectl.service"
if ! cmp -s contrib/phonectl.service "$unit"; then
  install -m 644 contrib/phonectl.service "$unit"
  systemctl --user daemon-reload
  echo "updated $unit"
fi
if [ "${1:-}" != "--no-restart" ]; then
  systemctl --user enable phonectl.service >/dev/null 2>&1 || true
  systemctl --user restart phonectl.service
  echo "restarted phonectl.service"
fi
if [ "${1:-}" = "--clean" ] || [ "${2:-}" = "--clean" ]; then
  cargo clean   # frees the build cache; the installed binary stays
fi
