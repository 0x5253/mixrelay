#!/usr/bin/env bash
set -euo pipefail
cd /home/qa/workspace
export XDG_RUNTIME_DIR="/run/user/$(id -u)"
export DBUS_SESSION_BUS_ADDRESS="unix:path=$XDG_RUNTIME_DIR/bus"
export XDG_SESSION_TYPE=x11 XDG_CURRENT_DESKTOP=MixRelay-QA DISPLAY=:99
export NO_AT_BRIDGE=0 GTK_MODULES=atk-bridge LANG=C.UTF-8
export PATH="/home/qa/tools:$PATH"
systemctl --user daemon-reload
systemctl --user start dbus.socket
systemctl --user import-environment DISPLAY XDG_SESSION_TYPE XDG_CURRENT_DESKTOP
dbus-update-activation-environment --systemd DISPLAY XDG_SESSION_TYPE XDG_CURRENT_DESKTOP
systemctl --user start mixrelay-qa-display.service pipewire.service wireplumber.service
systemctl --user is-active mixrelay-qa-display.service pipewire.service wireplumber.service
openbox --sm-disable > /home/qa/openbox.log 2>&1 &
window_manager=$!
trap 'kill "$window_manager" 2>/dev/null || true' EXIT
if [[ "$1" == --smoke ]]; then
  node --input-type=module - "$2" <<'JS'
import { observeEnvironment } from './scripts/e2e/environment.mjs';
import { validateEnvironment } from './scripts/core/environment-policy.mjs';
import { loadContract } from './scripts/support-contract.mjs';
import { mkdirSync, writeFileSync } from 'node:fs';
const { contract } = loadContract();
const target = contract.qualificationTargets.find(t => t.id === process.argv[2]);
const observed = observeEnvironment();
validateEnvironment(observed, target, contract);
mkdirSync('evidence');
writeFileSync('evidence/guest.host.json', JSON.stringify({ scope: 'host-readiness', targetId: target.id, observed }, null, 2) + '\n');
JS
else
  node scripts/e2e/installed.mjs --target "$1" --directory /home/qa/candidate \
    --candidate /home/qa/candidate/mix-relay.candidate.json --output evidence --disposable-environment
fi
