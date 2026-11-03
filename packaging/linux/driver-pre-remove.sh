#!/bin/sh
set -eu
# Debian's old package removal during replacement follows the preinstall drain.
if [ "${1:-}" = upgrade ]; then exit 0; fi
exec /bin/sh /usr/lib/mixrelay-driver/before-change.sh
