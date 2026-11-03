#!/usr/bin/env bash
set -euo pipefail
repository="$(pwd)"
if [[ "$1" == --prepare ]]; then
  mkdir "$2"
  tools="$(cd "$2" && pwd)"
  cp scripts/e2e/macos-ui/project.yml scripts/e2e/macos-ui/*.swift "$tools/"
  export MIXRELAY_UI_EVIDENCE="/unused-before-installed-test"
  cd "$tools"
  xcodegen generate --spec project.yml
  node "$repository/scripts/e2e/macos-ui-runner.mjs" --prepare "$tools"
else
  output="$(cd "$1" && pwd)"
  tools="${MIXRELAY_E2E_MACOS_UI_TOOLS:?Prepare the immutable UI runner before installation}"
  node "$repository/scripts/e2e/macos-ui-runner.mjs" "$output" "$tools"
fi
