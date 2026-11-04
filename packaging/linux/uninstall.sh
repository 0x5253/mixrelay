#!/bin/sh

# Parse the complete block before package removal can delete this script.
{
set -eu

case "${LC_ALL:-${LC_MESSAGES:-${LANG:-}}}" in
  ko*)
    title='MixRelay 제거'
    message='제거하기 전에 MixRelay 상태 메뉴에서 모든 오디오를 중지하고 MixRelay 편집 앱과 MixRelay를 종료하세요. 삭제를 선택하지 않으면 저장된 그래프와 설정은 유지됩니다. 녹음 파일과 내보낸 프로젝트는 항상 유지됩니다.'
    delete_label='저장된 그래프와 설정 삭제'
    remove_label='제거'
    cancel_label='취소'
    failed='제거하지 못했습니다. MixRelay를 종료하고 권한 요청을 승인한 뒤 다시 시도하세요. 저장된 데이터는 삭제하지 않았습니다.'
    unsafe='데이터 경로가 안전하지 않아 제거를 중단했습니다. 저장된 데이터는 삭제하지 않았습니다.'
    cleanup_failed='앱은 제거했지만 일부 그래프 또는 설정을 삭제하지 못했습니다.'
    root_error='로그인한 일반 사용자로 MixRelay 제거를 실행하세요.'
    missing='설치된 MixRelay 패키지를 찾지 못했습니다.'
    done_message='MixRelay를 제거했습니다.'
    ;;
  ja*)
    title='MixRelay のアンインストール'
    message='アンインストールする前に、MixRelayのステータスメニューですべてのオーディオを停止し、MixRelayエディターとMixRelayを終了してください。削除を選択しない限り、保存されたグラフと設定は保持されます。録音とエクスポートしたプロジェクトは常に保持されます。'
    delete_label='保存されたグラフと設定を削除'
    remove_label='アンインストール'
    cancel_label='キャンセル'
    failed='アンインストールできませんでした。MixRelay を終了し、権限要求を承認してから再試行してください。保存されたデータは削除していません。'
    unsafe='データのパスが安全でないため、アンインストールを中止しました。保存されたデータは削除していません。'
    cleanup_failed='アプリは削除されましたが、一部のグラフまたは設定を削除できませんでした。'
    root_error='ログイン中の一般ユーザーとして MixRelay のアンインストールを実行してください。'
    missing='インストール済みの MixRelay パッケージが見つかりませんでした。'
    done_message='MixRelay をアンインストールしました。'
    ;;
  zh_Hans*|zh-Hans*)
    title='卸载 MixRelay'
    message='卸载前，请从 MixRelay 状态菜单停止所有音频，并关闭 MixRelay 编辑器和 MixRelay。除非您选择删除，否则已保存的音频图和设置会保留。录音和导出的项目始终会保留。'
    delete_label='删除已保存的音频图和设置'
    remove_label='卸载'
    cancel_label='取消'
    failed='卸载失败。请退出 MixRelay，批准授权请求后重试。保存的数据未删除。'
    unsafe='数据路径不安全，已停止卸载。保存的数据未删除。'
    cleanup_failed='应用已卸载，但部分音频图或设置无法删除。'
    root_error='请以已登录的普通用户身份运行 MixRelay 卸载程序。'
    missing='未找到已安装的 MixRelay 软件包。'
    done_message='MixRelay 已卸载。'
    ;;
  zh_Hant*|zh-Hant*|zh_TW*|zh-TW*|zh_HK*|zh-HK*|zh_MO*|zh-MO*)
    title='解除安裝 MixRelay'
    message='解除安裝前，請從 MixRelay 狀態選單停止所有音訊，並關閉 MixRelay 編輯器與 MixRelay。除非您選擇刪除，否則已儲存的音訊圖與設定會保留。錄音與匯出的專案一律會保留。'
    delete_label='刪除已儲存的音訊圖與設定'
    remove_label='解除安裝'
    cancel_label='取消'
    failed='解除安裝失敗。請結束 MixRelay，核准授權要求後重試。儲存的資料未刪除。'
    unsafe='資料路徑不安全，已停止解除安裝。儲存的資料未刪除。'
    cleanup_failed='應用程式已移除，但部分音訊圖或設定無法刪除。'
    root_error='請以已登入的一般使用者身分執行 MixRelay 解除安裝程式。'
    missing='找不到已安裝的 MixRelay 套件。'
    done_message='MixRelay 已解除安裝。'
    ;;
  zh*)
    title='卸载 MixRelay'
    message='卸载前，请从 MixRelay 状态菜单停止所有音频，并关闭 MixRelay 编辑器和 MixRelay。除非您选择删除，否则已保存的音频图和设置会保留。录音和导出的项目始终会保留。'
    delete_label='删除已保存的音频图和设置'
    remove_label='卸载'
    cancel_label='取消'
    failed='卸载失败。请退出 MixRelay，批准授权请求后重试。保存的数据未删除。'
    unsafe='数据路径不安全，已停止卸载。保存的数据未删除。'
    cleanup_failed='应用已卸载，但部分音频图或设置无法删除。'
    root_error='请以已登录的普通用户身份运行 MixRelay 卸载程序。'
    missing='未找到已安装的 MixRelay 软件包。'
    done_message='MixRelay 已卸载。'
    ;;
  fr*)
    title='Désinstaller MixRelay'
    message='Avant de désinstaller, arrêtez tout l’audio depuis le menu d’état de MixRelay, puis fermez l’éditeur MixRelay et MixRelay. Les graphes et paramètres enregistrés sont conservés, sauf si vous choisissez de les supprimer. Les enregistrements et projets exportés sont toujours conservés.'
    delete_label='Supprimer les graphes et paramètres enregistrés'
    remove_label='Désinstaller'
    cancel_label='Annuler'
    failed='La désinstallation a échoué. Quittez MixRelay, approuvez la demande d’autorisation et réessayez. Les données enregistrées n’ont pas été supprimées.'
    unsafe='La désinstallation a été arrêtée, car un chemin de données n’est pas sûr. Les données enregistrées n’ont pas été supprimées.'
    cleanup_failed='L’application a été supprimée, mais certains graphes ou paramètres n’ont pas pu être supprimés.'
    root_error='Exécutez la désinstallation de MixRelay avec le compte utilisateur ordinaire connecté.'
    missing='Le paquet MixRelay installé est introuvable.'
    done_message='MixRelay a été désinstallé.'
    ;;
  de*)
    title='MixRelay deinstallieren'
    message='Stoppen Sie vor der Deinstallation alle Audiovorgänge im MixRelay-Statusmenü und schließen Sie den MixRelay-Editor und MixRelay. Gespeicherte Graphen und Einstellungen bleiben erhalten, sofern Sie nicht deren Löschung auswählen. Aufnahmen und exportierte Projekte bleiben immer erhalten.'
    delete_label='Gespeicherte Graphen und Einstellungen löschen'
    remove_label='Deinstallieren'
    cancel_label='Abbrechen'
    failed='Die Deinstallation ist fehlgeschlagen. Beenden Sie MixRelay, genehmigen Sie die Berechtigungsanfrage und versuchen Sie es erneut. Gespeicherte Daten wurden nicht gelöscht.'
    unsafe='Die Deinstallation wurde wegen eines unsicheren Datenpfads angehalten. Gespeicherte Daten wurden nicht gelöscht.'
    cleanup_failed='Die App wurde entfernt, aber einige Graphen oder Einstellungen konnten nicht gelöscht werden.'
    root_error='Führen Sie die Deinstallation von MixRelay als angemeldeter regulärer Benutzer aus.'
    missing='Das installierte MixRelay-Paket wurde nicht gefunden.'
    done_message='MixRelay wurde deinstalliert.'
    ;;
  es*)
    title='Desinstalar MixRelay'
    message='Antes de desinstalar, detenga todo el audio desde el menú de estado de MixRelay y cierre el editor de MixRelay y MixRelay. Los grafos y ajustes guardados se conservan, salvo que elija eliminarlos. Las grabaciones y los proyectos exportados se conservan siempre.'
    delete_label='Eliminar los grafos y ajustes guardados'
    remove_label='Desinstalar'
    cancel_label='Cancelar'
    failed='No se pudo desinstalar. Cierre MixRelay, apruebe la solicitud de autorización e inténtelo de nuevo. Los datos guardados no se han eliminado.'
    unsafe='Se detuvo la desinstalación porque una ruta de datos no es segura. Los datos guardados no se han eliminado.'
    cleanup_failed='Se eliminó la aplicación, pero no se pudieron eliminar algunos grafos o ajustes guardados.'
    root_error='Ejecute la desinstalación de MixRelay con el usuario normal que ha iniciado sesión.'
    missing='No se encontró el paquete de MixRelay instalado.'
    done_message='MixRelay se ha desinstalado.'
    ;;
  pt*)
    title='Desinstalar MixRelay'
    message='Antes de desinstalar, pare todo o áudio pelo menu de status do MixRelay e feche o editor do MixRelay e o MixRelay. Os grafos e as configurações salvos são mantidos, a menos que você escolha excluí-los. As gravações e os projetos exportados são sempre mantidos.'
    delete_label='Excluir grafos e configurações salvos'
    remove_label='Desinstalar'
    cancel_label='Cancelar'
    failed='A desinstalação falhou. Feche o MixRelay, aprove a solicitação de autorização e tente novamente. Os dados salvos não foram excluídos.'
    unsafe='A desinstalação foi interrompida porque um caminho de dados não é seguro. Os dados salvos não foram excluídos.'
    cleanup_failed='O aplicativo foi removido, mas não foi possível excluir alguns grafos ou configurações salvos.'
    root_error='Execute a desinstalação do MixRelay com o usuário comum que iniciou a sessão.'
    missing='O pacote MixRelay instalado não foi encontrado.'
    done_message='O MixRelay foi desinstalado.'
    ;;
  it*)
    title='Disinstalla MixRelay'
    message='Prima di disinstallare, interrompi tutto l’audio dal menu di stato di MixRelay e chiudi l’editor MixRelay e MixRelay. Grafi e impostazioni salvati vengono conservati, a meno che tu non scelga di eliminarli. Registrazioni e progetti esportati vengono sempre conservati.'
    delete_label='Elimina grafi e impostazioni salvati'
    remove_label='Disinstalla'
    cancel_label='Annulla'
    failed='Disinstallazione non riuscita. Chiudi MixRelay, approva la richiesta di autorizzazione e riprova. I dati salvati non sono stati eliminati.'
    unsafe='Disinstallazione interrotta perché un percorso dei dati non è sicuro. I dati salvati non sono stati eliminati.'
    cleanup_failed='L’applicazione è stata rimossa, ma non è stato possibile eliminare alcuni grafi o impostazioni salvati.'
    root_error='Esegui la disinstallazione di MixRelay con l’utente normale che ha effettuato l’accesso.'
    missing='Il pacchetto MixRelay installato non è stato trovato.'
    done_message='MixRelay è stato disinstallato.'
    ;;
  ru*)
    title='Удалить MixRelay'
    message='Перед удалением остановите всё аудио через меню состояния MixRelay и закройте редактор MixRelay и MixRelay. Сохранённые графы и настройки останутся, если вы не выберете их удаление. Записи и экспортированные проекты сохраняются всегда.'
    delete_label='Удалить сохранённые графы и настройки'
    remove_label='Удалить'
    cancel_label='Отмена'
    failed='Не удалось удалить приложение. Закройте MixRelay, одобрите запрос разрешения и повторите попытку. Сохранённые данные не удалены.'
    unsafe='Удаление остановлено, поскольку путь к данным небезопасен. Сохранённые данные не удалены.'
    cleanup_failed='Приложение удалено, но некоторые сохранённые графы или настройки удалить не удалось.'
    root_error='Запустите удаление MixRelay от имени обычного пользователя, вошедшего в систему.'
    missing='Установленный пакет MixRelay не найден.'
    done_message='MixRelay удалён.'
    ;;
  *)
    title='Uninstall MixRelay'
    message='Before uninstalling, stop all audio from the MixRelay status menu and close the MixRelay editor and MixRelay. Saved graphs and settings are kept unless you choose to delete them. Recordings and exported projects are always kept.'
    delete_label='Delete saved graphs and settings'
    remove_label='Uninstall'
    cancel_label='Cancel'
    failed='Uninstall failed. Quit MixRelay, approve the authorization request and retry. Saved data was not deleted.'
    unsafe='Uninstall stopped because a data path is unsafe. Saved data was not deleted.'
    cleanup_failed='The app was removed, but some saved graphs or settings could not be deleted.'
    root_error='Run Uninstall MixRelay as the signed-in regular user.'
    missing='The installed MixRelay package was not found.'
    done_message='MixRelay was uninstalled.'
    ;;
esac

show_error() {
  /usr/bin/zenity --error --title="$title" --text="$1" || :
  exit 1
}

if [ "$(/usr/bin/id -u)" = 0 ]; then
  show_error "$root_error"
fi

if [ -x /usr/bin/dpkg-query ] && [ "$(/usr/bin/dpkg-query -W -f='${db:Status-Status}' mix-relay 2>/dev/null || :)" = installed ]; then
  manager=apt
elif [ -x /usr/bin/rpm ] && /usr/bin/rpm -q mix-relay >/dev/null 2>&1; then
  manager=dnf
else
  show_error "$missing"
fi

selection=$(/usr/bin/zenity --list --checklist --title="$title" --text="$message" \
  --column='' --column="$delete_label" --hide-header --print-column=2 \
  --ok-label="$remove_label" --cancel-label="$cancel_label" FALSE "$delete_label") || exit 0

safe_path() {
  case "$1" in
    /*) ;;
    *) return 1 ;;
  esac
  case "$1" in
    *'/../'*|*'/./'*|*'//'*) return 1 ;;
  esac
  checked_path=$1
  while [ "$checked_path" != / ]; do
    [ ! -L "$checked_path" ] || return 1
    checked_path=${checked_path%/*}
    [ -n "$checked_path" ] || checked_path=/
  done
}

case "${HOME-}" in
  ''|/|/home|/root) show_error "$unsafe" ;;
  /*) ;;
  *) show_error "$unsafe" ;;
esac
case "${XDG_DATA_HOME-}" in
  /) show_error "$unsafe" ;;
  /*) data_base=$XDG_DATA_HOME ;;
  *) data_base=$HOME/.local/share ;;
esac
data=$data_base/io.mixrelay.desktop

if [ "$selection" = "$delete_label" ]; then
  case "${XDG_CONFIG_HOME-}" in
    /) show_error "$unsafe" ;;
    /*) config_base=$XDG_CONFIG_HOME ;;
    *) config_base=$HOME/.config ;;
  esac
  config=$config_base/mixrelay
  # Only fixed application-owned paths are eligible; exported files stay intact.
  set -- "$config/graphs.json" "$config/audio-session.json" "$config/graphs.states" \
    "$data/recording-settings.json" "$data/WebKitCache" "$data/MediaCache" \
    "$data/applications" "$data/CacheStorage"
  for path do
    safe_path "$path" || show_error "$unsafe"
  done
fi

case "$manager" in
  apt) /usr/bin/pkexec --disable-internal-agent /usr/bin/apt -y remove mix-relay || show_error "$failed" ;;
  dnf) /usr/bin/pkexec --disable-internal-agent /usr/bin/dnf --setopt=clean_requirements_on_remove=False -y remove mix-relay || show_error "$failed" ;;
esac

if [ "$selection" = "$delete_label" ]; then
  # Recheck after authorization/removal; no cleanup occurs on failure or cancel.
  for path do
    safe_path "$path" || show_error "$cleanup_failed"
  done
  /usr/bin/rm -f -- "$config/graphs.json" "$config/audio-session.json" \
    "$data/recording-settings.json" || show_error "$cleanup_failed"
  /usr/bin/rm -rf -- "$config/graphs.states" "$data/WebKitCache" \
    "$data/MediaCache" "$data/applications" "$data/CacheStorage" || show_error "$cleanup_failed"
fi

/usr/bin/zenity --info --title="$title" --text="$done_message" || :
exit 0
}
