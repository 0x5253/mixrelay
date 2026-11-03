import { act, render, screen } from '@testing-library/react';
import { useTranslation } from 'react-i18next';
import { afterEach, expect, it } from 'vitest';
import { browserLanguages } from '../adapters/browser/languages';
import { I18nProvider, i18n } from '.';
import { createI18n } from './createI18n';

afterEach(() => {
  document.documentElement.lang = 'en';
});

function Label() {
  const { t } = useTranslation();
  return <p>{t('theme.system')}</p>;
}

it('starts in the first supported preferred language and falls back to English', () => {
  expect(createI18n(['xx-YY', 'de-AT', 'ja']).language).toBe('de');
  expect(createI18n(['xx-YY']).language).toBe('en');
  expect(createI18n([]).language).toBe('en');
});

it('switches language synchronously, so the app can choose it right before rendering', () => {
  const instance = createI18n([]);
  void instance.changeLanguage('ja');
  expect(instance.language).toBe('ja');
  expect(instance.t('theme.system')).toBe(createI18n(['ja']).t('theme.system'));
});

it('keeps instances independent of each other and of the application instance', async () => {
  const before = i18n.language;
  const first = createI18n(['fr']);
  const second = createI18n(['fr']);
  await first.changeLanguage('ko-KR');
  expect([first.language, second.language, i18n.language]).toEqual(['ko-KR', 'fr', before]);
});

it('supplies a given instance and follows its language in the document', async () => {
  const instance = createI18n(['ja']);
  render(
    <I18nProvider instance={instance}>
      <Label />
    </I18nProvider>,
  );
  expect(document.documentElement).toHaveAttribute('lang', 'ja');
  expect(screen.getByText(instance.t('theme.system'))).toBeInTheDocument();
  await act(async () => {
    await instance.changeLanguage('es');
  });
  expect(document.documentElement).toHaveAttribute('lang', 'es');
});

it('reads the browser preferences in order, then the primary language', () => {
  expect(browserLanguages({ languages: ['pt-BR', 'pt'], language: 'en-US' })).toEqual([
    'pt-BR',
    'pt',
    'en-US',
  ]);
});
