#!/bin/zsh
# Measures latency, bit exactness, channel order, slips and dropouts of an audio path on
# installed devices (any vendor). See native/macos/hal-driver/tests/MRRoundTripProbe.c.
#   pnpm probe:macos-roundtrip --latency DEVICE_UID (read-only reported latency)
#   pnpm probe:macos-roundtrip --transport DEVICE_UID
#   pnpm probe:macos-roundtrip --bundle BUNDLE_ID --transport DEVICE_UID
#     (runs the probe from an ad-hoc signed app bundle with that identifier, so the HAL
#      sees a bundled requester; unsigned bundles are reported by macOS as damaged)
#   pnpm probe:macos-roundtrip --list
#   pnpm probe:macos-roundtrip [--json] [--seconds S] [--channels N] OUTPUT_UID [INPUT_UID]
#   pnpm probe:macos-roundtrip --self-test
#   pnpm probe:macos-roundtrip --prepare-bundle BUNDLE_ID
#     (prints a signed probe app path for a LaunchServices permission test)
# GUI recording resolves microphone consent before opening devices; a nongraphical CLI
# retains CoreAudio's native broker. Denied or unresolved consent exits with status 2.
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  print -u2 "the round-trip probe only runs on macOS"
  exit 1
fi
repo_root="${0:A:h:h}"
minimum_macos="$(node "$repo_root/scripts/macos-minimum.mjs")"
source_file="$repo_root/native/macos/hal-driver/tests/MRRoundTripProbe.c"
permission_source="$repo_root/native/macos/service/src/MRMicrophonePermission.m"
activity_source="$repo_root/native/macos/hal-driver/tests/MRProbeActivity.m"
trace_source="$repo_root/native/macos/hal-driver/tests/MRProbeHalTrace.c"
trace_header="${trace_source%.c}.h"
authorization_source="$repo_root/native/macos/hal-driver/tests/MRProbeAuthorization.c"
authorization_header="${authorization_source%.c}.h"
binary="$repo_root/target/macos-probes/MRRoundTripProbe"
if [[ ! -x "$binary" || "$source_file" -nt "$binary" || "$permission_source" -nt "$binary" || "$activity_source" -nt "$binary" || "$trace_source" -nt "$binary" || "$trace_header" -nt "$binary" || "$authorization_source" -nt "$binary" || "$authorization_header" -nt "$binary" ]]; then
  mkdir -p "${binary:h}"
  clang -std=c17 -O2 "-mmacosx-version-min=$minimum_macos" \
    -Wall -Wextra -Wpedantic -Wconversion -Wsign-conversion -Wshadow -Werror \
    -fblocks "$source_file" "$permission_source" "$activity_source" "$trace_source" "$authorization_source" -framework CoreAudio -framework CoreFoundation \
    -framework AVFoundation -framework Security -framework Foundation -o "$binary"
fi
prepare_bundle=false
if [[ "${1:-}" == "--prepare-bundle" ]]; then
  prepare_bundle=true
fi
if [[ "${1:-}" == "--bundle" || "$prepare_bundle" == true ]]; then
  bundle_id="${2:?--bundle needs a bundle identifier}"
  shift 2
  [[ "$bundle_id" =~ '^[A-Za-z0-9.-]+$' ]] || { print -u2 "invalid bundle identifier"; exit 2; }
  bundle="${binary:h}/$bundle_id.app"
  rm -rf "$bundle"
  mkdir -p "$bundle/Contents/MacOS"
  cp "$binary" "$bundle/Contents/MacOS/MRRoundTripProbe"
  cat > "$bundle/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>$bundle_id</string>
<key>CFBundleExecutable</key><string>MRRoundTripProbe</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>NSMicrophoneUsageDescription</key><string>Verify audio recorded from the MixRelay test device.</string>
</dict></plist>
PLIST
  codesign --force --sign - "$bundle"
  binary="$bundle/Contents/MacOS/MRRoundTripProbe"
  if [[ "$prepare_bundle" == true ]]; then
    print -r -- "$bundle"
    exit 0
  fi
fi
exec "$binary" "$@"
