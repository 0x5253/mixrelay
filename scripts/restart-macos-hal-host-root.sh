#!/bin/zsh
# Reload one verified plugin host after a successful bundle replacement. Some
# CoreAudio plugin hosts ignore SIGTERM, so bound the graceful shutdown attempt.
set -euo pipefail
[[ "$(uname -s)" == Darwin && "$(id -u)" == 0 && $# == 2 ]] || exit 2
driver_pid="$1"
mapped_bundle="$2"
[[ "$driver_pid" == <-> && "$mapped_bundle" == /Library/Audio/Plug-Ins/HAL/MixRelayHAL.driver* ]] || exit 2
[[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$mapped_bundle/Contents/Info.plist")" == io.mixrelay.audio.driver ]] || exit 3
validate_pid() {
  [[ "$(/bin/ps -p "$driver_pid" -o comm=)" == 'Core Audio Driver (MixRelayHAL.driver)' ]] || return 1
  /usr/sbin/lsof -p "$driver_pid" -Fn | /usr/bin/grep -Fx "n$mapped_bundle/Contents/MacOS/MixRelayHAL" >/dev/null
}
validate_pid || exit 4
/bin/kill -TERM "$driver_pid"
for attempt in {1..30}; do
  if ! /bin/kill -0 "$driver_pid" 2>/dev/null; then
    print "MixRelay HAL host $driver_pid exited after SIGTERM"
    exit 0
  fi
  sleep 0.1
done
# Recheck mapped bundle and identity immediately before escalating; no process
# name matching, global kill, coreaudiod restart or unrelated driver signal.
validate_pid || exit 4
/bin/kill -KILL "$driver_pid"
for attempt in {1..30}; do
  if ! /bin/kill -0 "$driver_pid" 2>/dev/null; then
    print "MixRelay HAL host $driver_pid ignored SIGTERM; exited after targeted SIGKILL"
    exit 0
  fi
  sleep 0.1
done
print -u2 "MixRelay HAL host $driver_pid termination not confirmed"
exit 5
