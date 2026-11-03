import { render, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fakeNativeShell } from '../../test-support/fakeNativeShell';
import { NativeMenuLocale } from './NativeMenuLocale';

const translation = vi.hoisted(() => ({
  i18n: { resolvedLanguage: 'ko-KR' as string | undefined, language: 'ko-KR' },
}));
vi.mock('react-i18next', () => ({ useTranslation: () => ({ i18n: translation.i18n }) }));

describe('NativeMenuLocale', () => {
  beforeEach(() => {
    translation.i18n.resolvedLanguage = 'ko-KR';
  });

  it('sends the resolved UI language and updates when it changes', () => {
    const native = fakeNativeShell();
    const view = render(<NativeMenuLocale />, { wrapper: native.Provider });
    expect(native.shell.setMenuLocale).toHaveBeenLastCalledWith('ko-KR');
    translation.i18n.resolvedLanguage = 'en';
    view.rerender(<NativeMenuLocale />);
    expect(native.shell.setMenuLocale).toHaveBeenLastCalledWith('en');
  });

  it('does nothing without a native shell', () => {
    expect(() => render(<NativeMenuLocale />)).not.toThrow();
  });

  it('sends the requested language before a resource has been resolved', () => {
    const native = fakeNativeShell();
    translation.i18n.resolvedLanguage = undefined;
    translation.i18n.language = 'pt-BR';
    render(<NativeMenuLocale />, { wrapper: native.Provider });
    expect(native.shell.setMenuLocale).toHaveBeenLastCalledWith('pt-BR');
  });

  it.each(['ja', 'zh-Hans', 'zh-Hant', 'fr', 'de', 'es', 'pt', 'it', 'ru'])(
    'sends %s to native menus without substituting English',
    (locale) => {
      const native = fakeNativeShell();
      translation.i18n.resolvedLanguage = locale;
      render(<NativeMenuLocale />, { wrapper: native.Provider });
      expect(native.shell.setMenuLocale).toHaveBeenLastCalledWith(locale);
    },
  );

  it('reports native failures without an unhandled rejection', async () => {
    const native = fakeNativeShell();
    const log = vi.spyOn(console, 'error').mockImplementation(() => {});
    native.shell.setMenuLocale.mockRejectedValue('native menu unavailable');
    render(<NativeMenuLocale />, { wrapper: native.Provider });
    await waitFor(() =>
      expect(log).toHaveBeenCalledWith(
        'Could not update native menu language',
        'native menu unavailable',
      ),
    );
    log.mockRestore();
  });
});
