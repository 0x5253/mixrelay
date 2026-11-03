#!/bin/zsh
set -euo pipefail

# Opt-in fixture only. Exact MixRelay virtual UID; no physical/default routing,
# installed graph mutation, permission reset, or user-application capture.
repo_root="${0:A:h:h}"
minimum_macos="$(node "$repo_root/scripts/macos-minimum.mjs")"
fixture_root="${1:?provide a dedicated output directory}"
endpoint_id="${2:?provide an existing test-owned canonical endpoint UUID}"
mkdir -p "$fixture_root/MRVirtualToneSource.app/Contents/MacOS"
native_root="$repo_root/native/macos/application-capture"
cp "$native_root/tests/MRVirtualToneSource-Info.plist" "$fixture_root/MRVirtualToneSource.app/Contents/Info.plist"
clang -fobjc-arc -fmodules -fblocks "-mmacosx-version-min=$minimum_macos" \
  -Wall -Wextra -Wpedantic -Wconversion -Wsign-conversion -Wshadow -Werror \
  "$native_root/tests/MRVirtualToneSource.m" \
  -framework AVFAudio -framework Foundation -framework CoreAudio -framework AudioUnit \
  -o "$fixture_root/MRVirtualToneSource.app/Contents/MacOS/MRVirtualToneSource"
cd "$repo_root"
MIXRELAY_APPLICATION_CLOCK_FIXTURE=1 node scripts/lab/native-check.mjs cargo test --release -p mixrelay-desktop \
  live_owned_application_tap_v2_src_rate_changes --no-run
if [[ "${3:-}" == "--run" ]]; then
  MIXRELAY_APPLICATION_CLOCK_FIXTURE=1 \
  MIXRELAY_VIRTUAL_TONE_FIXTURE="$fixture_root/MRVirtualToneSource.app/Contents/MacOS/MRVirtualToneSource" \
  MIXRELAY_VIRTUAL_TONE_ENDPOINT_ID="$endpoint_id" \
    node scripts/lab/native-check.mjs cargo test --release -p mixrelay-desktop live_owned_application_tap_v2_src_rate_changes -- --nocapture
fi
print "Fixture artifacts retained: $fixture_root"
