import { render, screen } from '@testing-library/react';
import i18next from 'i18next';
import { useLocale } from 'react-aria-components';
import { I18nextProvider } from 'react-i18next';
import { expect, it } from 'vitest';

import { MixRelayProvider } from './MixRelayProvider';
import { mixrelayVariables, themeRoles } from './tokens';

it('publishes shared tokens to app and Storybook consumers', () => {
  const { container } = render(
    <MixRelayProvider>
      <span>MixRelay</span>
    </MixRelayProvider>,
  );
  const css = Array.from(container.querySelectorAll('style[data-mixrelay-theme]'))
    .map((style) => style.textContent)
    .join('\n');
  expect(css).toContain(`--canvas-background:${mixrelayVariables['--canvas-background']}`);
  expect(css).toContain(`--status-accent:${themeRoles.light['--status-accent']}`);
  expect(css).toContain(`--status-accent:${themeRoles.dark['--status-accent']}`);
  expect(css).toContain(`--meter-normal:${themeRoles.dark['--meter-normal']}`);
  // Legacy aliases are gone; consumers use the canonical roles directly.
  expect(css).not.toMatch(/--(bg|text|muted|line|red|amber|positive|accent|signal):/u);
  expect(css).toContain('--transport-background-active:var(--status-danger-surface)');
  expect(css).toContain(`--meter-high-start:${mixrelayVariables['--meter-high-start']}`);
  expect(css).toContain('--transport-size:var(--control-height)');
  expect(css).toContain('--form-actions-gap:var(--space-2)');
  expect(css).toContain('--form-actions-inset:var(--space-3)');
});

it('formats in English while the i18n instance has not resolved a language', () => {
  function LocaleProbe() {
    return <span>{useLocale().locale}</span>;
  }
  const unresolved = i18next.createInstance();
  void unresolved.init({ react: { useSuspense: false } });
  expect(unresolved.resolvedLanguage).toBeUndefined();
  render(
    <I18nextProvider i18n={unresolved}>
      <MixRelayProvider>
        <LocaleProbe />
      </MixRelayProvider>
    </I18nextProvider>,
  );
  expect(screen.getByText('en')).toBeInTheDocument();
});
