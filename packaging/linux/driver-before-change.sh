#!/bin/sh
set -eu

case "${LC_ALL:-${LC_MESSAGES:-${LANG:-}}}" in
  en*) instruction='Stop all audio from the MixRelay status menu, close the MixRelay editor and MixRelay, then retry.' ;;
  ko*) instruction='MixRelay 상태 메뉴에서 모든 오디오를 중지하고 MixRelay 편집 앱과 MixRelay를 종료한 뒤 다시 시도하세요.' ;;
  ja*) instruction='MixRelayのステータスメニューですべてのオーディオを停止し、MixRelayエディターとMixRelayを終了してから再試行してください。' ;;
  zh_Hans*|zh-Hans*) instruction='请从 MixRelay 状态菜单停止所有音频，关闭 MixRelay 编辑器和 MixRelay，然后重试。' ;;
  zh_Hant*|zh-Hant*|zh_TW*|zh-TW*|zh_HK*|zh-HK*|zh_MO*|zh-MO*) instruction='請從 MixRelay 狀態選單停止所有音訊，關閉 MixRelay 編輯器與 MixRelay，然後重試。' ;;
  zh*) instruction='请从 MixRelay 状态菜单停止所有音频，关闭 MixRelay 编辑器和 MixRelay，然后重试。' ;;
  fr*) instruction='Arrêtez tout l’audio depuis le menu d’état de MixRelay, fermez l’éditeur MixRelay et MixRelay, puis réessayez.' ;;
  de*) instruction='Stoppen Sie alle Audiovorgänge im MixRelay-Statusmenü, schließen Sie den MixRelay-Editor und MixRelay und versuchen Sie es erneut.' ;;
  es*) instruction='Detenga todo el audio desde el menú de estado de MixRelay, cierre el editor de MixRelay y MixRelay y vuelva a intentarlo.' ;;
  pt*) instruction='Pare todo o áudio pelo menu de estado do MixRelay, feche o editor do MixRelay e o MixRelay e tente novamente.' ;;
  it*) instruction='Interrompi tutto l’audio dal menu di stato di MixRelay, chiudi l’editor MixRelay e MixRelay e riprova.' ;;
  ru*) instruction='Остановите всё аудио через меню состояния MixRelay, закройте редактор MixRelay и MixRelay, затем повторите попытку.' ;;
  *) instruction='Stop all audio from the MixRelay status menu, close the MixRelay editor and MixRelay, then retry.' ;;
esac

# Ask installed editors and controllers to drain through their normal exit path.
# Never kill an engine: its controller lease closes after accepted work is drained.
for executable in /proc/[0-9]*/exe; do
  target=$(readlink "$executable" 2>/dev/null) || continue
  case "$target" in
    /usr/bin/mixrelay-desktop|'/usr/bin/mixrelay-desktop (deleted)') ;;
    *) continue ;;
  esac
  process=${executable%/exe}
  uid=$(stat -c %u "$process")
  [ "$uid" -ne 0 ] || { printf '%s\n' 'MixRelay is incorrectly running as root.' >&2; exit 1; }
  /bin/kill -TERM "${process##*/}" 2>/dev/null || [ ! -e "$executable" ]
done

# Allow acknowledged engine/recorder finalization, retaining files on failure.
# Thirty seconds bounds the package transaction; expiration never escalates to KILL.
attempts=0
while :; do
  running=false
  for executable in /proc/[0-9]*/exe; do
    target=$(readlink "$executable" 2>/dev/null) || continue
    case "$target" in
      /usr/bin/mixrelay-desktop|'/usr/bin/mixrelay-desktop (deleted)'|\
      /usr/bin/mixrelay-service|'/usr/bin/mixrelay-service (deleted)') running=true ;;
    esac
  done
  [ "$running" = true ] || break
  attempts=$((attempts + 1))
  [ "$attempts" -lt 300 ] || { printf '%s\n' "$instruction" >&2; exit 1; }
  sleep 0.1
done

# Resolve only actual installed Driver processes. Audio remains in their user session.
for executable in /proc/[0-9]*/exe; do
  target=$(readlink "$executable" 2>/dev/null) || continue
  case "$target" in
    /usr/bin/mixrelay-driver-service|'/usr/bin/mixrelay-driver-service (deleted)') ;;
    *) continue ;;
  esac
  process=${executable%/exe}
  uid=$(stat -c %u "$process")
  [ "$uid" -ne 0 ] || { printf '%s\n' 'MixRelay Driver is incorrectly running as root.' >&2; exit 1; }
  account=$(getent passwd "$uid")
  user=${account%%:*}
  [ -n "$user" ] || exit 1
  runtime="/run/user/$uid"
  [ "$(stat -c %u "$runtime")" = "$uid" ] && [ ! -L "$runtime" ] || exit 1
  runuser -u "$user" -- env XDG_RUNTIME_DIR="$runtime" DBUS_SESSION_BUS_ADDRESS="unix:path=$runtime/bus" \
    systemctl --user stop mixrelay-driver.service
  attempts=0
  while [ -e "$process/exe" ]; do
    attempts=$((attempts + 1))
    [ "$attempts" -lt 50 ] || { printf '%s\n' 'MixRelay Driver did not drain after service stop.' >&2; exit 1; }
    sleep 0.1
  done
done
