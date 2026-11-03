import type { i18n as I18n } from 'i18next';
import { type PropsWithChildren, useEffect } from 'react';
import { I18nextProvider, initReactI18next } from 'react-i18next';
import { createI18n } from './createI18n';
import { resolveLocale } from './locale';

export type { SupportedLocale } from './locale';
export { formattingLocale, resolveLocale, supportedLocales } from './locale';

/** The application's translations. They start in English; the composition root switches
 * them to the user's language before the first render. */
export const i18n = createI18n([], initReactI18next);

/** Supplies `instance` (the application's by default) and keeps the document language,
 * which assistive technology reads, on its current language. */
export function I18nProvider({
  children,
  instance = i18n,
}: PropsWithChildren<{ instance?: I18n }>) {
  useEffect(() => {
    const updateDocumentLanguage = () => {
      document.documentElement.lang = resolveLocale(instance.resolvedLanguage ?? instance.language);
    };
    updateDocumentLanguage();
    instance.on('languageChanged', updateDocumentLanguage);
    return () => {
      instance.off('languageChanged', updateDocumentLanguage);
    };
  }, [instance]);
  return <I18nextProvider i18n={instance}>{children}</I18nextProvider>;
}
