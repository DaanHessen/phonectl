#!/usr/bin/env bash
# Builds phonectl Keyboard: HeliBoard at a pinned tag plus patches/*.patch.
# Output: keyboard/phonectl-keyboard.apk (signed with ~/.android/debug.keystore,
# the same key as the phonectl app; the bridge permission is signature-level).
set -euo pipefail
cd "$(dirname "$0")"
TAG=v4.2
REPO=https://github.com/HeliBorg/HeliBoard

if [ ! -d upstream/.git ]; then
  git clone --depth 1 --branch "$TAG" "$REPO" upstream
fi
cd upstream
if [ "$(git describe --tags 2>/dev/null)" != "$TAG" ]; then
  echo "upstream/ is not at $TAG; remove keyboard/upstream and run again" >&2
  exit 1
fi
# Apply the patch unless it is already in the tree (patch = source of truth).
if git apply --check ../patches/*.patch 2>/dev/null; then
  git apply ../patches/*.patch
elif ! git apply --reverse --check ../patches/*.patch 2>/dev/null; then
  echo "upstream/ has other changes; reset it to $TAG first" >&2
  exit 1
fi
[ -f local.properties ] || echo "sdk.dir=${ANDROID_HOME:-$HOME/sdk/android}" > local.properties
rm -f app/build/outputs/apk/release/*.apk
nice -n 10 ./gradlew --no-daemon --max-workers=4 assembleRelease
apk=$(ls app/build/outputs/apk/release/*-release.apk | head -1)
# Must match the phonectl app's key, or the bridge permission is not granted.
# (AGP takes the debug key from $XDG_CONFIG_HOME/.android when that is set, so
# a sandboxed shell silently signs with a new key.)
expected=32bae3c60bcaa36d06e2489c76b43f831ee14949da107b6d3945acb6b93fb63c
sdk=${ANDROID_HOME:-$HOME/sdk/android}
signer=$(ls -d "$sdk"/build-tools/*/ | sort -V | tail -1)apksigner
if ! "$signer" verify --print-certs "$apk" | grep -q "SHA-256 digest: $expected"; then
  echo "built APK is not signed with the phonectl key ($expected); check XDG_CONFIG_HOME / ~/.android" >&2
  exit 1
fi
cp "$apk" ../phonectl-keyboard.apk
echo "built $(cd .. && pwd)/phonectl-keyboard.apk"
