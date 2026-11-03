import { afterEach, describe, expect, it } from 'vitest';
import { i18n } from '.';
import { formattingLocale, resolveLocale, supportedLocales } from './locale';
import { de } from './locales/de';
import { en } from './locales/en';
import { es } from './locales/es';
import { fr } from './locales/fr';
import { it as italian } from './locales/it';
import { ja } from './locales/ja';
import { koKR } from './locales/ko-KR';
import { pseudo } from './locales/pseudo';
import { pt } from './locales/pt';
import { ru } from './locales/ru';
import { zhHans } from './locales/zh-Hans';
import { zhHant } from './locales/zh-Hant';
import { pseudoResource } from './pseudoResource';

const translations = {
  en,
  'ko-KR': koKR,
  ja,
  'zh-Hans': zhHans,
  'zh-Hant': zhHant,
  fr,
  de,
  es,
  pt,
  it: italian,
  ru,
};

function flatten(resource: object, prefix = ''): Record<string, string> {
  const result: Record<string, string> = {};
  for (const [key, value] of Object.entries(resource)) {
    const path = prefix ? `${prefix}.${key}` : key;
    if (typeof value === 'string') result[path] = value;
    else Object.assign(result, flatten(value, path));
  }
  return result;
}

afterEach(async () => {
  await i18n.changeLanguage('en');
});

it('fills every Pseudo key without English fallback and preserves interpolation tokens', () => {
  const base = flatten(en);
  const expanded = flatten(pseudo);
  expect(Object.keys(expanded).sort()).toEqual(Object.keys(base).sort());
  for (const [key, text] of Object.entries(base)) {
    expect(expanded[key]?.length).toBeGreaterThan(0);
    expect(expanded[key]?.match(/\{\{[^}]+\}\}/g) ?? []).toEqual(
      text.match(/\{\{[^}]+\}\}/g) ?? [],
    );
  }
  for (const key of [
    'studio.deleteGraph',
    'studio.deleteGraphNotice',
    'updates.title',
    'updates.installed',
  ])
    expect(expanded[key]).not.toBe(base[key]);
  expect(pseudoResource({ action: 'Save {{name}}' })).toEqual({
    action: '[!! Save {{name}} — expanded translation !!]',
  });
});

describe('language preference resolution', () => {
  it.each([
    ['en-US', 'en'],
    ['EN_gb', 'en'],
    ['ko', 'ko-KR'],
    ['ko-KR', 'ko-KR'],
    ['ja-JP', 'ja'],
    ['fr-FR', 'fr'],
    ['fr-CA', 'fr'],
    ['de-DE', 'de'],
    ['de-AT', 'de'],
    ['es-ES', 'es'],
    ['es-MX', 'es'],
    ['pt-BR', 'pt'],
    ['pt-PT', 'pt'],
    ['it-IT', 'it'],
    ['ru-RU', 'ru'],
    ['zh', 'zh-Hans'],
    ['zh-CN', 'zh-Hans'],
    ['zh-SG', 'zh-Hans'],
    ['zh-Hans', 'zh-Hans'],
    ['zh-TW', 'zh-Hant'],
    ['zh-HK', 'zh-Hant'],
    ['zh-MO', 'zh-Hant'],
    ['zh-Hant', 'zh-Hant'],
    ['zh-Hant-CN', 'zh-Hant'],
    ['zh-Hans-TW', 'zh-Hans'],
    ['franken', 'en'],
    ['unknown', 'en'],
    ['', 'en'],
  ])('resolves %s to %s', (input, expected) => {
    expect(resolveLocale(input)).toBe(expected);
  });

  it('respects ordered preferences and skips unsupported languages before falling back', () => {
    expect(resolveLocale(['nl-NL', 'ja-JP', 'en'])).toBe('ja');
    expect(resolveLocale(['es-ES', 'ja-JP', 'en'])).toBe('es');
    expect(resolveLocale(['zh-HK', 'zh-CN'])).toBe('zh-Hant');
    expect(resolveLocale(['en-US', 'fr-FR'])).toBe('en');
    expect(resolveLocale(['nl-NL', 'pl-PL'])).toBe('en');
    expect(resolveLocale([])).toBe('en');
  });
});

describe.each(supportedLocales)('%s complete translation resource', (locale) => {
  it('covers every source key and preserves every interpolation argument', () => {
    const source = flatten(en);
    const translated = flatten(translations[locale]);
    expect(Object.keys(translated).sort()).toEqual(Object.keys(source).sort());
    for (const [key, value] of Object.entries(translated)) {
      expect(value.trim(), key).not.toBe('');
      const placeholders = (text: string) => (text.match(/{{\s*\w+\s*}}/g) ?? []).sort();
      const original = source[key];
      if (original === undefined) throw new Error(`Missing source translation: ${key}`);
      expect(placeholders(value), key).toEqual(placeholders(original));
    }
    if (locale !== 'en') {
      expect(
        Object.keys(source).filter((key) => translated[key] !== source[key]).length,
      ).toBeGreaterThan(300);
    }
  });

  it('selects the real resource instead of English fallback', async () => {
    await i18n.changeLanguage(locale);
    expect(i18n.resolvedLanguage).toBe(locale);
    expect(i18n.t('driverManagement.title')).toBe(translations[locale].driverManagement.title);
    expect(i18n.t('recorder.chooseFolder')).toBe(translations[locale].recorder.chooseFolder);
    expect(i18n.t('updates.installed', { version: '1.2.3' })).toBe(
      translations[locale].updates.installed.replace('{{version}}', '1.2.3'),
    );
  });

  it('uses the corresponding Intl date locale', () => {
    const intlLocale = formattingLocale(locale);
    const formatter = new Intl.DateTimeFormat(intlLocale, { dateStyle: 'medium', timeZone: 'UTC' });
    expect(formatter.resolvedOptions().locale).toBe(intlLocale);
    expect(formatter.format(new Date('2026-10-08T00:00:00Z'))).toBeTruthy();
    if (locale !== 'en')
      expect(formatter.format(new Date('2026-10-08T00:00:00Z'))).not.toBe(
        new Intl.DateTimeFormat('en', { dateStyle: 'medium', timeZone: 'UTC' }).format(
          new Date('2026-10-08T00:00:00Z'),
        ),
      );
  });
});
