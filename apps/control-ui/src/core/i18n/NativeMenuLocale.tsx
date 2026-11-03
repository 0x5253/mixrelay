import { useContext, useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import { NativeShellContext } from '../shell/NativeShellContext';

/** Keeps the native menus in the editor's language. */
export function NativeMenuLocale() {
  const shell = useContext(NativeShellContext);
  const { i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language;

  useEffect(() => {
    void shell?.setMenuLocale(locale).catch((error: unknown) => {
      console.error('Could not update native menu language', error);
    });
  }, [shell, locale]);

  return null;
}
