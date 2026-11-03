#!/bin/zsh
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  print -u2 "MixRelay development audio installation only runs on macOS"
  exit 1
fi

repo_root="${0:A:h:h}"
build_root="$repo_root/target/macos-dev-audio"
driver_root="$build_root/hal"
driver_bundle="$driver_root/MixRelayHAL.driver"
service_binary="$repo_root/target/debug/mixrelay-service"
service_plist="$build_root/io.mixrelay.audio.engine.plist"
root_installer="$repo_root/scripts/install-macos-dev-audio-root.sh"
service_user="$(id -un)"
service_group="$(id -gn)"
service_home="$HOME"
service_log_root="$service_home/Library/Logs/MixRelay"
service_stdout="$service_log_root/engine.stdout.log"
service_stderr="$service_log_root/engine.stderr.log"

mkdir -p "$build_root" "$service_log_root"
cargo run --manifest-path "$repo_root/Cargo.toml" -p mixrelay-hal-bundle -- --output "$driver_root"
cargo build --manifest-path "$repo_root/Cargo.toml" -p mixrelay-service

plutil -create xml1 "$service_plist"
plutil -insert Label -string "io.mixrelay.audio.engine" "$service_plist"
plutil -insert ProgramArguments -json "[\"$service_binary\"]" "$service_plist"
plutil -insert MachServices -json '{"io.mixrelay.audio.engine":true}' "$service_plist"
plutil -insert ProcessType -string Interactive "$service_plist"
plutil -insert UserName -string "$service_user" "$service_plist"
plutil -insert GroupName -string "$service_group" "$service_plist"
plutil -insert EnvironmentVariables -json \
  "{\"HOME\":\"$service_home\",\"USER\":\"$service_user\",\"LOGNAME\":\"$service_user\"}" \
  "$service_plist"
plutil -insert RunAtLoad -bool false "$service_plist"
plutil -insert KeepAlive -bool false "$service_plist"
plutil -insert StandardOutPath -string "$service_stdout" "$service_plist"
plutil -insert StandardErrorPath -string "$service_stderr" "$service_plist"
plutil -lint "$service_plist" >/dev/null

admin_command="$(printf '%q ' "$root_installer" "$driver_bundle" "$service_binary" "$service_plist")"
/usr/bin/osascript \
  -e 'on run argv' \
  -e 'do shell script (item 1 of argv) with administrator privileges' \
  -e 'end run' \
  -- "$admin_command"

print "MixRelay development audio is installed"
print "Processor host user: $service_user"
print "Engine stdout: $service_stdout"
print "Engine stderr: $service_stderr"
