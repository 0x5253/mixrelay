#!/usr/bin/env bash
set -euo pipefail

# UTM guest-exec may return before a long process exits. The caller must read
# exit-code, never interpret utmctl's immediate exit status as job completion.
job_id=${1:?Expected unique job ID}
shift
[[ "$job_id" =~ ^[a-zA-Z0-9_-]+$ ]] || exit 2
(( $# > 0 )) || exit 2
repo_root=$(cd "$(dirname "$0")/../.." && pwd)
job_root=${MIXRELAY_LAB_JOB_ROOT:-$repo_root/.lab/jobs}
[[ "$job_root" == /* ]] || exit 2
job_dir="$job_root/mixrelay-lab-$job_id"
umask 077
mkdir -p "$job_root"
mkdir "$job_dir" # Exclusive: never overwrite evidence from a previous run.
exec >"$job_dir/output.log" 2>&1
result=0
trap 'result=$?; printf "%s\n" "$result" >"$job_dir/exit-code.tmp"; mv "$job_dir/exit-code.tmp" "$job_dir/exit-code"' EXIT
child_pid=
stop_child() {
  local status=$1 pid _
  trap '' INT TERM
  while read -r pid; do
    if [[ "$pid" == "$child_pid" ]]; then
      kill -TERM "$child_pid" 2>/dev/null || true
      for _ in {1..20}; do
        [[ $(jobs -pr) == "$child_pid" ]] || break
        sleep 0.1
      done
      [[ $(jobs -pr) != "$child_pid" ]] || kill -KILL "$child_pid" 2>/dev/null || true
      wait "$child_pid" 2>/dev/null || true
      break
    fi
  done < <(jobs -pr)
  exit "$status"
}
trap 'stop_child 130' INT
trap 'stop_child 143' TERM
"$@" <&0 &
child_pid=$!
wait "$child_pid"
