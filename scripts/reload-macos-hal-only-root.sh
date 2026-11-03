#!/bin/zsh
# Development HAL replacement only. No launchd/security/default-device changes.
set -euo pipefail
[[ "$(uname -s)" == Darwin && "$(id -u)" == 0 && $# == 2 ]] || exit 2
bundle="$1"
driver_pid="$2"
installed=/Library/Audio/Plug-Ins/HAL/MixRelayHAL.driver
[[ "$bundle" == /* && "$driver_pid" == <-> && -d "$bundle" ]] || exit 2
for candidate in "$bundle" "$installed"; do
  [[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$candidate/Contents/Info.plist")" == io.mixrelay.audio.driver ]] || exit 3
  /usr/bin/codesign --verify --strict "$candidate"
done
validate_pid() {
  [[ "$(/bin/ps -p "$driver_pid" -o comm=)" == 'Core Audio Driver (MixRelayHAL.driver)' ]] || return 1
  # This process must actually map the exact installed MixRelay bundle.
  /usr/sbin/lsof -p "$driver_pid" -Fn | /usr/bin/grep -Fx "n$installed/Contents/MacOS/MixRelayHAL" >/dev/null
}
validate_pid || exit 4
suffix="$(date -u +%Y%m%dT%H%M%SZ)-$$"
stage="$installed.stage-$suffix"
backup="$installed.backup-$suffix"
[[ ! -e "$stage" && ! -e "$backup" ]] || exit 5
/usr/bin/ditto "$bundle" "$stage"
/usr/sbin/chown -R root:wheel "$stage"
/bin/chmod -R go-w "$stage"
/usr/bin/codesign --verify --strict "$stage"
validate_pid || exit 4
/bin/mv "$installed" "$backup"
if ! /bin/mv "$stage" "$installed"; then
  /bin/mv "$backup" "$installed"
  exit 6
fi
print "Previous MixRelay HAL preserved: $backup"
# Verify termination, not merely successful delivery of a signal. The old host
# now maps the preserved backup inode after the directory rename.
/bin/zsh "${0:A:h}/restart-macos-hal-host-root.sh" "$driver_pid" "$backup"
