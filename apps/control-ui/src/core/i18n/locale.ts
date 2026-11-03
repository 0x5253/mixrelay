export const supportedLocales = [
  'en',
  'ko-KR',
  'ja',
  'zh-Hans',
  'zh-Hant',
  'fr',
  'de',
  'es',
  'pt',
  'it',
  'ru',
] as const;
export type SupportedLocale = (typeof supportedLocales)[number];

function supportedLocale(language: string): SupportedLocale | undefined {
  const parts = language.trim().replaceAll('_', '-').toLowerCase().split('-');
  switch (parts[0]) {
    case 'en':
      return 'en';
    case 'ko':
      return 'ko-KR';
    case 'ja':
      return 'ja';
    case 'fr':
      return 'fr';
    case 'de':
      return 'de';
    case 'es':
      return 'es';
    case 'pt':
      return 'pt';
    case 'it':
      return 'it';
    case 'ru':
      return 'ru';
    case 'zh': {
      // An explicit script takes precedence over a conflicting regional hint.
      if (parts.includes('hant')) return 'zh-Hant';
      if (parts.includes('hans')) return 'zh-Hans';
      return parts.some((part) => ['tw', 'hk', 'mo'].includes(part)) ? 'zh-Hant' : 'zh-Hans';
    }
    default:
      return undefined;
  }
}

/** Choose the first supported OS/browser preference; use English when none match. */
export function resolveLocale(language: string | readonly string[]): SupportedLocale {
  for (const candidate of typeof language === 'string' ? [language] : language) {
    const locale = supportedLocale(candidate);
    if (locale) return locale;
  }
  return 'en';
}

/** Concrete regional locales keep date/number formatting in the selected UI language. */
export function formattingLocale(language: string): string {
  const locale = resolveLocale(language);
  if (locale === 'zh-Hans') return 'zh-CN';
  if (locale === 'zh-Hant') return 'zh-TW';
  return locale;
}
