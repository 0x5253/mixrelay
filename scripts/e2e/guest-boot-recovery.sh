#!/usr/bin/env bash
set -euo pipefail

# ARM TCG can finish coldplug after systemd's initial device jobs expire.
# Retry the image's unchanged mount units only after its real devices are ready.
if ! systemctl --quiet is-active emergency.target; then
  exit 0
fi
printf '%s\n' 'Recovering Ubuntu guest coldplug after emergency boot'
udevadm trigger --action=add --subsystem-match=block
udevadm settle --timeout=120
for label in BOOT UEFI; do
  device=$(blkid -L "$label")
  test -n "$device"
  printf '%s: %s\n' "$label" "$device"
done
systemctl reset-failed boot.mount boot-efi.mount
systemctl start boot.mount boot-efi.mount
for directory in /boot /boot/efi; do
  findmnt --mountpoint "$directory"
done
systemctl --no-block default
systemctl --no-block start cloud-config.service cloud-final.service
