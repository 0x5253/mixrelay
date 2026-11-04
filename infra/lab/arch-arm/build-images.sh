#!/usr/bin/env bash
# Builds the Arch Linux ARM images the arm64 Arch checks use from the signed upstream
# rootfs: a container rootfs for code checks and a UEFI cloud-init disk for installed
# qualification. Arch Linux publishes neither for aarch64.
#
# Usage: infra/lab/arch-arm/build-images.sh ROOTFS.tar.gz ROOTFS.tar.gz.sig EMPTY_OUTPUT_DIR
# Needs an arm64 Docker host (Apple silicon or an arm64 Linux runner) and gpg.
set -euo pipefail

# Arch Linux ARM Build System <builder@archlinuxarm.org>
readonly SIGNER=68B3537F39A313B3E574D06777193F152BDBE6A6
# Fixed partition identities, so the boot entry and fstab need no lookup at first boot.
readonly ESP_UUID=6d1b2f2e-3a4c-4f0e-9d7a-1c5e8b2a9f01
readonly ROOT_UUID=b4c1d2e3-5f60-4a7b-8c9d-0e1f2a3b4c5d
readonly ESP_MIB=1024

if [[ $# -ne 3 ]]; then
  echo "Usage: $0 ROOTFS.tar.gz ROOTFS.tar.gz.sig EMPTY_OUTPUT_DIR" >&2
  exit 2
fi
rootfs=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
signature=$(cd "$(dirname "$2")" && pwd)/$(basename "$2")
output=$3
here=$(cd "$(dirname "$0")" && pwd)
[[ $(docker version --format '{{.Server.Arch}}') == arm64 ]] || {
  echo 'An arm64 Docker host is required' >&2
  exit 1
}
mkdir -p "$output"
[[ -z $(ls -A "$output") ]] || {
  echo 'Output directory must be empty' >&2
  exit 1
}
output=$(cd "$output" && pwd)

keyring=$(mktemp -d)
work=$(docker volume create)
cleanup() {
  rm -rf "$keyring"
  docker rm -f mixrelay-alarm-base mixrelay-alarm-vm mixrelay-alarm-container >/dev/null 2>&1 || true
  docker volume rm -f "$work" >/dev/null 2>&1 || true
  docker image rm -f mixrelay-alarm:raw mixrelay-alarm:base >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo '==> Verifying the upstream rootfs signature'
GNUPGHOME=$keyring gpg --batch --quiet --keyserver hkps://keyserver.ubuntu.com --recv-keys "$SIGNER" ||
  curl -fsSL "https://keyserver.ubuntu.com/pks/lookup?op=get&search=0x$SIGNER" |
  GNUPGHOME=$keyring gpg --batch --quiet --import
GNUPGHOME=$keyring gpg --batch --status-fd 1 --verify "$signature" "$rootfs" 2>/dev/null |
  grep -q "^\[GNUPG:\] VALIDSIG $SIGNER " || {
  echo 'Rootfs signature is not from the Arch Linux ARM build key' >&2
  exit 1
}
stamp=$(date -u +%Y%m%d)

echo '==> Updating the upstream rootfs'
docker import "$rootfs" mixrelay-alarm:raw >/dev/null
# Docker's seccomp profile refuses pacman's Landlock sandbox, so builds run unsandboxed.
docker run --name mixrelay-alarm-base mixrelay-alarm:raw /usr/bin/bash -euc '
  pacman-key --init
  pacman-key --populate archlinuxarm
  pacman --disable-sandbox -Syu --noconfirm'
docker commit mixrelay-alarm-base mixrelay-alarm:base >/dev/null

echo '==> Container rootfs'
# A container needs no kernel, firmware or boot loader. The keyring is generated per
# host by the lab image instead of shipping one master key to every user.
docker run --name mixrelay-alarm-container mixrelay-alarm:base /usr/bin/bash -euc '
  pacman --disable-sandbox -Rdd --noconfirm linux-aarch64 $(pacman -Qq | grep "^linux-firmware")
  rm -rf /var/cache/pacman/pkg/*
  sed -i "s/^\[options\]$/&\nDisableSandbox/" /etc/pacman.conf
  grep -qx DisableSandbox /etc/pacman.conf
  rm -rf /etc/pacman.d/gnupg /var/lib/pacman/sync/* /boot/*
  rm -f /etc/machine-id'
docker export mixrelay-alarm-container | zstd -T0 -19 -q -o "$output/archlinuxarm-container-$stamp.tar.zst"

echo '==> Cloud image root'
docker run --name mixrelay-alarm-vm -v "$here/guest:/opt/guest:ro" mixrelay-alarm:base /usr/bin/bash -euc '
  pacman --disable-sandbox -S --noconfirm --needed cloud-init cloud-guest-utils sudo openssh
  # A virtual machine loads no device firmware.
  pacman --disable-sandbox -Rdd --noconfirm $(pacman -Qq | grep "^linux-firmware")
  install -m 0644 /opt/guest/pacman-init.service /etc/systemd/system/pacman-init.service
  install -m 0644 /opt/guest/mkinitcpio.conf /etc/mkinitcpio.conf
  install -m 0644 /opt/guest/cloud.cfg /etc/cloud/cloud.cfg.d/90-mixrelay-nocloud.cfg
  install -m 0644 /opt/guest/ds-identify.cfg /etc/cloud/ds-identify.cfg
  mkinitcpio -P
  # The upstream image has fixed root/alarm passwords; only the cloud-init user logs in.
  userdel -r alarm
  passwd -l root
  systemctl enable pacman-init.service sshd.service systemd-networkd.service systemd-resolved.service
  for unit in cloud-init-main cloud-init-local cloud-init-network cloud-init cloud-config cloud-final; do
    if [[ -e /usr/lib/systemd/system/$unit.service ]]; then systemctl enable "$unit.service"; fi
  done
  rm -rf /var/cache/pacman/pkg/* /etc/pacman.d/gnupg /var/lib/pacman/sync/* /etc/ssh/ssh_host_*
  : > /etc/machine-id'

echo '==> Assembling the UEFI disk'
docker export mixrelay-alarm-vm |
  docker run --rm -i -v "$work:/work" -v "$here/guest:/opt/guest:ro" mixrelay-alarm:base /usr/bin/bash -euc "
  pacman --disable-sandbox -S --noconfirm --needed dosfstools mtools e2fsprogs qemu-img >/dev/null
  mkdir /work/root /work/esp
  bsdtar -xpf - --numeric-owner --xattrs -C /work/root
  # The ESP is mounted at /boot, so kernel upgrades in the guest replace the booted files.
  mv /work/root/boot/* /work/esp/
  mkdir -p /work/esp/EFI/BOOT /work/esp/loader/entries
  cp /work/root/usr/lib/systemd/boot/efi/systemd-bootaa64.efi /work/esp/EFI/BOOT/BOOTAA64.EFI
  install -m 0644 /opt/guest/loader.conf /work/esp/loader/loader.conf
  sed 's/@ROOT_UUID@/$ROOT_UUID/' /opt/guest/arch.conf > /work/esp/loader/entries/arch.conf
  sed -e 's/@ROOT_UUID@/$ROOT_UUID/' -e 's/@ESP_UUID@/$ESP_UUID/' /opt/guest/fstab > /work/root/etc/fstab
  # Docker bind-mounts these into every container, so the export holds its placeholders.
  ln -sf ../run/systemd/resolve/stub-resolv.conf /work/root/etc/resolv.conf
  printf '127.0.0.1 localhost\n::1 localhost\n' > /work/root/etc/hosts
  : > /work/root/etc/hostname
  truncate -s ${ESP_MIB}M /work/esp.img
  mkfs.vfat -F 32 -n ESP /work/esp.img >/dev/null
  mcopy -s -i /work/esp.img /work/esp/* ::/
  root_mib=\$(( \$(du -sm /work/root | cut -f1) + 1536 ))
  mkfs.ext4 -q -L root -d /work/root /work/root.img \${root_mib}M
  truncate -s \$(( 1 + ${ESP_MIB} + root_mib + 1 ))M /work/disk.raw
  printf '%s\n' 'label: gpt' \
    'start=1MiB, size=${ESP_MIB}MiB, type=C12A7328-F81F-11D2-BA4B-00A0C93EC93B, uuid=$ESP_UUID, name=ESP' \
    \"size=\${root_mib}MiB, type=B921B045-1DF0-41C3-AF44-4C6F280D3FAE, uuid=$ROOT_UUID, name=root\" |
    sfdisk -q /work/disk.raw
  dd if=/work/esp.img of=/work/disk.raw bs=1M seek=1 conv=notrunc,sparse status=none
  dd if=/work/root.img of=/work/disk.raw bs=1M seek=\$(( 1 + ${ESP_MIB} )) conv=notrunc,sparse status=none
  qemu-img convert -c -O qcow2 /work/disk.raw /work/cloudimg.qcow2"
docker run --rm -v "$work:/work:ro" -v "$output:/output" mixrelay-alarm:base \
  cp /work/cloudimg.qcow2 "/output/archlinuxarm-cloudimg-$stamp.qcow2"

(cd "$output" && shasum -a 256 ./*.tar.zst ./*.qcow2 | sed 's# \./# #' > SHA256SUMS)
cat "$output/SHA256SUMS"
