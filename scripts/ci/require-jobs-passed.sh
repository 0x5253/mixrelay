#!/usr/bin/env bash
# Required-check gate: passes only when every other job of this run attempt finished with
# success or skipped. A matrix leg that never got a runner is not reported through `needs`,
# and a reusable workflow's jobs can still read as in progress just after `needs` releases
# this job, so wait up to five minutes for them to settle before judging.
set -euo pipefail
: "${SELF:?name of the calling job}"
jobs="$(mktemp)"
for _ in $(seq 30); do
  gh api "repos/$GITHUB_REPOSITORY/actions/runs/$GITHUB_RUN_ID/attempts/$GITHUB_RUN_ATTEMPT/jobs" --paginate \
    --jq '.jobs[] | select(.name != env.SELF) | "\(.name)\t\(.status)\t\(.conclusion)"' > "$jobs"
  awk -F'\t' '$2 != "completed"' "$jobs" | grep -q . || break
  sleep 10
done
cat "$jobs"
! awk -F'\t' '$2 != "completed" || ($3 != "success" && $3 != "skipped")' "$jobs" | grep .
