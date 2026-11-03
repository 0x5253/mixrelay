#!/bin/zsh
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  print -u2 "MixRelay installed duplex smoke only runs on macOS"
  exit 1
fi

if (( $# != 2 )); then
  print -u2 "usage: $0 <canonical-uid> <expected-gain: 0|0.5|1>"
  exit 1
fi
uid_pattern='^io\.mixrelay\.audio\.endpoint\.[0-9a-f]{32}$'
if [[ ! "$1" =~ "$uid_pattern" || "$1" == "io.mixrelay.audio.endpoint.00000000000000000000000000000000" || \
      ( "$2" != 0 && "$2" != 0.5 && "$2" != 1 ) ]]; then
  print -u2 "explicit canonical UID and expected gain 0, 0.5 or 1 required"
  exit 1
fi

repo_root="${0:A:h:h}"
minimum_macos="$(node "$repo_root/scripts/macos-minimum.mjs")"
mkdir -p "$repo_root/target/native-tests"
smoke_root="$(mktemp -d "$repo_root/target/native-tests/installed-duplex.XXXXXX")"
smoke_binary="$smoke_root/MRInstalledDuplexSmoke"
service_target="system/io.mixrelay.audio.driver"
service_plist="/Library/LaunchDaemons/io.mixrelay.audio.driver.plist"

cleanup() {
  rm -rf -- "$smoke_root"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

clang \
  -std=c17 \
  "-mmacosx-version-min=$minimum_macos" \
  -Wall \
  -Wextra \
  -Wpedantic \
  -Wconversion \
  -Wsign-conversion \
  -Wshadow \
  -Werror \
  "$repo_root/native/macos/hal-driver/tests/MRInstalledDuplexSmoke.c" \
  -framework CoreAudio \
  -framework CoreFoundation \
  -o "$smoke_binary"

for attempt in {1..20}; do
  set +e
  smoke_output="$($smoke_binary "$@" 2>&1)"
  smoke_status=$?
  set -e
  if (( smoke_status == 0 )); then
    print -r -- "$smoke_output"
    exit 0
  fi
  if (( smoke_status != 2 )); then
    print -u2 -r -- "$smoke_output"
    break
  fi
  sleep 1
done

print -u2 "MixRelay installed duplex smoke failed"
/bin/launchctl print "$service_target" 2>&1 | sed -n '1,160p' >&2 || true
for log_key in StandardOutPath StandardErrorPath; do
  service_log="$(/usr/libexec/PlistBuddy -c "Print :$log_key" "$service_plist" 2>/dev/null || true)"
  if [[ -n "$service_log" && -f "$service_log" ]]; then
    tail -n 160 "$service_log" >&2
  fi
done
/usr/bin/log show --last 2m --style compact \
  --predicate 'process == "mixrelay-driver-service" OR (process == "coreaudiod" AND eventMessage CONTAINS[c] "MixRelay")' \
  2>&1 | tail -n 160 >&2 || true
exit "${smoke_status:-1}"
