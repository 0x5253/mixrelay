#!/usr/bin/env bash
set -euo pipefail

# Used inside a disposable build image or explicitly in a development VM, never
# invoked on the host by the lab CLI. Rolling distributions get a full upgrade.
source /etc/os-release
case "$ID" in
  debian|ubuntu|kali)
    export DEBIAN_FRONTEND=noninteractive
    apt_options=(-o APT::Update::Error-Mode=any)
    if [[ "$ID" == kali && "${MIXRELAY_LAB_CONTAINER:-0}" == 1 ]]; then
      # The rotating redirector selected a mirror with an invalid TLS chain.
      # Use Kali's official endpoint in disposable images without weakening TLS
      # or archive signature checks, and without changing a VM's configured repos.
      apt_options+=(-o Dir::Etc::sourcelist=/opt/mixrelay-lab/kali.sources -o Dir::Etc::sourceparts=-
        -o Acquire::https::CaInfo=/etc/ssl/certs/ca-certificates.crt)
    fi
    apt-get "${apt_options[@]}" update
    apt-get "${apt_options[@]}" install -y --no-install-recommends \
      build-essential ca-certificates clang libclang-rt-dev cmake curl git pkg-config rsync ripgrep \
      libasound2-dev liblilv-dev lv2-dev libpipewire-0.3-dev libssl-dev \
      libgtk-3-dev libwebkit2gtk-4.1-dev libayatana-appindicator3-dev \
      librsvg2-dev libx11-dev libxrandr-dev libxi-dev \
      pipewire pipewire-bin wireplumber dbus-user-session dbus-x11 xvfb xauth
    ;;
  arch)
    pacman -Syu --noconfirm --needed \
      base-devel ca-certificates clang compiler-rt cmake curl git pkgconf openssl rsync ripgrep \
      alsa-lib lilv lv2 pipewire wireplumber gtk3 webkit2gtk-4.1 \
      libayatana-appindicator librsvg libx11 libxrandr libxi \
      dbus xorg-server-xvfb xorg-xauth
    ;;
  *) echo "Unsupported distribution: $ID" >&2; exit 2 ;;
esac
