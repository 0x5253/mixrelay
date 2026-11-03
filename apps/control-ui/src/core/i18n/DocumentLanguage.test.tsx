import { act, cleanup, render } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { I18nProvider, i18n, supportedLocales } from '.';

afterEach(async () => {
  cleanup();
  await i18n.changeLanguage('en');
  document.documentElement.lang = 'en';
});

it.each(supportedLocales)('sets the document language to %s at mount', async (locale) => {
  await i18n.changeLanguage(locale);
  document.documentElement.lang = 'en';
  render(
    <I18nProvider>
      <div />
    </I18nProvider>,
  );
  expect(document.documentElement).toHaveAttribute('lang', locale);
});

it('updates accessibility language on language changes and removes its listener on unmount', async () => {
  const view = render(
    <I18nProvider>
      <div />
    </I18nProvider>,
  );
  await act(async () => {
    await i18n.changeLanguage('zh-Hant');
  });
  expect(document.documentElement).toHaveAttribute('lang', 'zh-Hant');
  await act(async () => {
    await i18n.changeLanguage('ru');
  });
  expect(document.documentElement).toHaveAttribute('lang', 'ru');
  view.unmount();
  await i18n.changeLanguage('de');
  expect(document.documentElement).toHaveAttribute('lang', 'ru');
});

it('uses English document language for the development pseudo locale', async () => {
  await i18n.changeLanguage('pseudo');
  render(
    <I18nProvider>
      <div />
    </I18nProvider>,
  );
  expect(document.documentElement).toHaveAttribute('lang', 'en');
});

it('uses the requested language before i18next has resolved its resource', () => {
  i18n.resolvedLanguage = undefined;
  i18n.language = 'ja-JP';
  render(
    <I18nProvider>
      <div />
    </I18nProvider>,
  );
  expect(document.documentElement).toHaveAttribute('lang', 'ja');
});
