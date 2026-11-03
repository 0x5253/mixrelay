#!/bin/sh
set -eu

case "${LC_ALL:-${LC_MESSAGES:-${LANG:-}}}" in
  en*) instruction='Run MixRelay as the signed-in user.' ;;
  ko*) instruction='로그인한 사용자로 MixRelay를 실행하세요.' ;;
  ja*) instruction='ログイン中のユーザーとしてMixRelayを実行してください。' ;;
  zh_Hans*|zh-Hans*) instruction='请以已登录的用户身份运行 MixRelay。' ;;
  zh_Hant*|zh-Hant*|zh_TW*|zh-TW*|zh_HK*|zh-HK*|zh_MO*|zh-MO*) instruction='請以已登入的使用者身分執行 MixRelay。' ;;
  zh*) instruction='请以已登录的用户身份运行 MixRelay。' ;;
  fr*) instruction='Exécutez MixRelay avec le compte utilisateur connecté.' ;;
  de*) instruction='Führen Sie MixRelay als angemeldeter Benutzer aus.' ;;
  es*) instruction='Ejecute MixRelay con el usuario que ha iniciado sesión.' ;;
  pt*) instruction='Execute o MixRelay como o utilizador com sessão iniciada.' ;;
  it*) instruction='Eseguire MixRelay con l’utente che ha effettuato l’accesso.' ;;
  ru*) instruction='Запустите MixRelay от имени пользователя, вошедшего в систему.' ;;
  *) instruction='Run MixRelay as the signed-in user.' ;;
esac
[ "$(id -u)" -ne 0 ] || { printf '%s\n' "$instruction" >&2; exit 1; }
[ "$#" -eq 0 ] || exit 2
systemctl --user start mixrelay-driver.service
