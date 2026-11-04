#!/usr/bin/env bash
set -euo pipefail

# Run only inside the disposable cloud guest, before installing the candidate.
source /etc/os-release
if [[ "$ID" == arch ]]; then
  pacman -Syu --noconfirm --needed xorg-server-xvfb dbus pipewire pipewire-audio \
    pipewire-pulse wireplumber glib2 libarchive openbox sudo python-atspi python-gobject gtk3 gdb strace
elif [[ "$ID" == fedora ]]; then
  dnf install -y --setopt=install_weak_deps=False xorg-x11-server-Xvfb dbus-daemon dbus-x11 pipewire \
    pipewire-utils pipewire-pulseaudio wireplumber glib2 bsdtar openbox sudo python3-pyatspi \
    python3-gobject gtk3 gdb strace
else
  export DEBIAN_FRONTEND=noninteractive
  apt-get update
  apt-get install -y --no-install-recommends xvfb dbus-x11 dbus-user-session pipewire pipewire-bin \
    pipewire-pulse wireplumber libglib2.0-bin libarchive-tools openbox sudo python3-pyatspi gir1.2-gtk-3.0 gdb strace
fi
command -v gdbus >/dev/null
loginctl enable-linger qa
install -d -m 0700 -o qa -g qa /home/qa/.config /home/qa/.config/systemd /home/qa/.config/systemd/user
sudo -u qa test -w /home/qa/.config
stat -c '%U:%G %a %n' /home/qa/.config /home/qa/.config/systemd /home/qa/.config/systemd/user
cat > /home/qa/.config/systemd/user/mixrelay-qa-display.service <<'UNIT'
[Unit]
Description=Disposable qualification X11 display
[Service]
ExecStart=/usr/bin/Xvfb :99 -screen 0 1440x900x24 -nolisten tcp
[Install]
WantedBy=default.target
UNIT
chown qa:qa /home/qa/.config/systemd/user/mixrelay-qa-display.service
