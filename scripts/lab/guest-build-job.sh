#!/usr/bin/env bash
set -euo pipefail
# Build-only limits. Never use this wrapper for audio timing qualification.
[[ $(uname -s) == Linux && $(id -u) == 0 ]] || exit 2
[[ $(hostname) == mixrelay-debian-engine ]] || exit 2
job_id=${1:?Expected unique job ID}
shift
[[ "$job_id" =~ ^[a-zA-Z0-9_-]+$ ]] || exit 2
(( $# > 0 )) || exit 2
workspace=$(cd "$(dirname "$0")/../.." && pwd)
[[ "$workspace" == /home/lab/.lab/mixrelay-integration-* ]] || exit 2
# The transient unit limits all descendants together, not merely the Cargo
# parent. It neither changes lab's account-wide policy nor survives reboot.
# systemd does not load shell profiles. Resolve the existing lab rustup proxies
# with lab's own home; never inherit the root caller's HOME or toolchain PATH.
exec systemd-run --unit="mixrelay-build-$job_id" --wait --collect \
    --service-type=exec --uid=lab --working-directory="$workspace" \
    --setenv=HOME=/home/lab \
    --setenv=PATH=/home/lab/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    --property=CPUQuota=100% --property=MemoryHigh=2G --property=MemoryMax=3G \
    --property=MemorySwapMax=0 --property=TasksMax=128 --property=Nice=10 \
    --property=IOWeight=10 --property='IOWriteBandwidthMax=/dev/vda 16M' \
    --property=RuntimeMaxSec=30min --property=TimeoutStopSec=15s \
    --property=KillMode=control-group \
    /bin/bash "$workspace/scripts/lab/guest-job.sh" "$job_id" "$@"
