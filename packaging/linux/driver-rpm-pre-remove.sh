#!/bin/sh
set -eu
if [ "$1" -gt 0 ]; then exit 0; fi
exec /bin/sh /usr/lib/mixrelay-driver/before-change.sh
