#!/bin/zsh
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" || "$(id -u)" != "0" ]]; then
  print -u2 "MixRelay development audio installation requires macOS administrator privileges"
  exit 1
fi

if (( $# != 3 )); then
  print -u2 "usage: $0 <driver-bundle> <service-binary> <launch-daemon-plist>"
  exit 1
fi

source_bundle="$1"
service_binary="$2"
source_plist="$3"
installed_bundle="/Library/Audio/Plug-Ins/HAL/MixRelayHAL.driver"
installed_plist="/Library/LaunchDaemons/io.mixrelay.audio.engine.plist"
service_target="system/io.mixrelay.audio.engine"
lifecycle_root="/Library/Application Support/MixRelay/lifecycle"

if [[ ! -d "$source_bundle" || ! -x "$service_binary" || ! -f "$source_plist" ]]; then
  print -u2 "MixRelay development audio artifacts are incomplete"
  exit 1
fi
if [[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$source_bundle/Contents/Info.plist")" != "io.mixrelay.audio.driver" ]]; then
  print -u2 "refusing to install a bundle with an unexpected identifier"
  exit 1
fi
if [[ "$(/usr/libexec/PlistBuddy -c 'Print :Label' "$source_plist")" != "io.mixrelay.audio.engine" ]]; then
  print -u2 "refusing to install a launch daemon with an unexpected label"
  exit 1
fi

# Validate all incoming artifacts before replacing the exact installed paths.
# Development builds remain reproducible; do not accumulate timestamped copies
# of drivers or launch configuration inside system discovery directories.
/usr/bin/codesign --verify --strict "$source_bundle"
/usr/bin/plutil -lint "$source_plist" >/dev/null
service_user="$(/usr/libexec/PlistBuddy -c 'Print :UserName' "$source_plist")"
if [[ "$(/usr/libexec/PlistBuddy -c 'Print :KeepAlive' "$source_plist")" != false || \
      "$(/usr/libexec/PlistBuddy -c 'Print :RunAtLoad' "$source_plist")" != false ]]; then
  print -u2 "Engine must be app-owned, not an always-running launch daemon"
  exit 1
fi
if ! /usr/bin/id "$service_user" >/dev/null 2>&1; then
  print -u2 "Engine service user does not exist"
  exit 1
fi

# Root owns the containing directories; the selected desktop user can lock but
# cannot replace these fixed inodes. Other HAL hosts may only probe read locks.
for directory in "/Library/Application Support/MixRelay" "$lifecycle_root"; do
  if [[ -L "$directory" || ( -e "$directory" && ! -d "$directory" ) ]]; then
    print -u2 "Refusing invalid lifecycle directory: $directory"
    exit 1
  fi
done
/usr/bin/install -d -o root -g wheel -m 0755 "/Library/Application Support/MixRelay" "$lifecycle_root"
for lease in "$lifecycle_root/app.owner" "$lifecycle_root/engine.owner"; do
  if [[ -L "$lease" || ( -e "$lease" && ! -f "$lease" ) ]]; then
    print -u2 "Refusing invalid lifecycle inode: $lease"
    exit 1
  fi
  if [[ ! -e "$lease" ]]; then
    /usr/bin/install -o "$service_user" -g wheel -m 0644 /dev/null "$lease"
  fi
  /usr/sbin/chown "$service_user":wheel "$lease"
  /bin/chmod 0644 "$lease"
done

/usr/bin/ditto "$source_bundle" "$installed_bundle"
/usr/sbin/chown -R root:wheel "$installed_bundle"
/bin/chmod -R go-w "$installed_bundle"
/usr/bin/codesign --verify --strict "$installed_bundle"
/usr/bin/ditto "$source_plist" "$installed_plist"
/usr/sbin/chown root:wheel "$installed_plist"
/bin/chmod 0644 "$installed_plist"

/bin/launchctl bootout "$service_target" >/dev/null 2>&1 || true
# launchd can still be tearing down the old registration after bootout returns.
# Retry only this exact bootstrap; never repeat the installation on EIO.
bootstrap_ok=false
bootstrap_error=""
for attempt in {1..10}; do
  if bootstrap_error="$(/bin/launchctl bootstrap system "$installed_plist" 2>&1)"; then
    bootstrap_ok=true
    break
  fi
  /bin/sleep 0.5
done
if [[ "$bootstrap_ok" != true ]]; then
  print -u2 "Engine registration failed: $bootstrap_error"
  print -u2 "Installed artifacts remain available; no user settings were removed."
  exit 1
fi
/usr/bin/killall coreaudiod >/dev/null 2>&1 || true

print "Installed MixRelayHAL at $installed_bundle"
print "Registered MixRelay engine service at $service_target"
print "The engine starts with MixRelay and exits when desktop ownership is released"
