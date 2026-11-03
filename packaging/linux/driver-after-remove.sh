#!/bin/sh
set -eu
# Drop cached definitions in existing user managers after the package removes the unit.
for runtime in /run/user/[0-9]*; do
  [ -d "$runtime" ] && [ ! -L "$runtime" ] && [ -S "$runtime/bus" ] || continue
  uid=${runtime##*/}
  [ "$uid" -ne 0 ] && [ "$(stat -c %u "$runtime")" = "$uid" ] || continue
  account=$(getent passwd "$uid") || continue
  user=${account%%:*}
  [ -n "$user" ] || continue
  runuser -u "$user" -- env XDG_RUNTIME_DIR="$runtime" DBUS_SESSION_BUS_ADDRESS="unix:path=$runtime/bus" \
    systemctl --user daemon-reload
done
