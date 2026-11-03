#!/usr/bin/env bash
set -euo pipefail
fixture=${1:?Expected repository fixture directory}
lab=${2:?Expected newly created private lab directory}
hook=${3:-none}
[[ "$hook" == none || "$hook" == source-mute ]] || exit 2
[[ "$lab" == /*/.lab/mixrelay-integration-*/target/native-tests/*-lab-* &&
   -d "$lab/config" && ! -e "$lab/config/wireplumber" ]] || exit 2
version=$(wireplumber --version)
# Ubuntu 24.04 and Debian 13 ship different external configuration grammars.
# Select from the installed version; never retry a failed daemon with another policy.
if [[ "$version" == *'0.4.'* ]]; then
    [[ "$hook" == none ]] || {
        printf 'Forward source-mute requires WirePlumber 0.5; installed version is unsupported\n' >&2
        exit 2
    }
    cp -R /usr/share/wireplumber "$lab/config/wireplumber"
    cp "$fixture/application-lab-wireplumber.conf" "$lab/config/wireplumber/wireplumber.conf"
    cp "$fixture/85-mixrelay-lab.lua" "$lab/config/wireplumber/main.lua.d/85-mixrelay-lab.lua"
elif [[ "$version" == *'0.5.'* ]]; then
    cp -R /usr/share/wireplumber "$lab/config/wireplumber"
    mkdir -p "$lab/config/wireplumber/wireplumber.conf.d"
    cp "$fixture/application-lab-wireplumber-0.5.conf" "$lab/config/wireplumber/wireplumber.conf.d/85-mixrelay-lab.conf"
    if [[ "$hook" == source-mute ]]; then
        hook_source=$(cd -- "$fixture/../../wireplumber" && pwd -P)
        mkdir -p "$lab/data/wireplumber"
        # WirePlumber 0.5.8 DATA_DIR scripts live under scripts/, separately
        # from CONFIG_DIR fragments. Keep standard scripts in this private copy.
        cp -R "$lab/config/wireplumber/scripts" "$lab/data/wireplumber/scripts"
        cp "$hook_source/90-mixrelay-source-mute.conf" "$lab/config/wireplumber/wireplumber.conf.d/90-mixrelay-source-mute.conf"
        cp "$hook_source/mixrelay-source-mute.lua" "$lab/data/wireplumber/scripts/mixrelay-source-mute.lua"
        cmp "$hook_source/90-mixrelay-source-mute.conf" "$lab/config/wireplumber/wireplumber.conf.d/90-mixrelay-source-mute.conf"
        cmp "$hook_source/mixrelay-source-mute.lua" "$lab/data/wireplumber/scripts/mixrelay-source-mute.lua"
        sha256sum "$hook_source/90-mixrelay-source-mute.conf" "$hook_source/mixrelay-source-mute.lua" \
            "$lab/config/wireplumber/wireplumber.conf.d/90-mixrelay-source-mute.conf" \
            "$lab/data/wireplumber/scripts/mixrelay-source-mute.lua" >"$lab/source-mute-hook.sha256"
    fi
else
    printf 'Unqualified WirePlumber configuration version: %s\n' "$version" >&2
    exit 2
fi
printf '%s\n' "$version"
