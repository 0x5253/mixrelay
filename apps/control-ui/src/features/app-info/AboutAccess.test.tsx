import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { expect, it } from 'vitest';
import sourceLicense from '../../../../../LICENSE.txt?raw';
import { I18nProvider, i18n } from '../../core/i18n';
import { MixRelayProvider } from '../../core/theme/MixRelayProvider';
import { AboutAccess } from './AboutAccess';

it('opens product information from the version and restores focus when closed', async () => {
  await i18n.changeLanguage('en');
  render(
    <MixRelayProvider>
      <I18nProvider>
        <AboutAccess>
          {(action) => (
            <main>
              <p>Workspace</p>
              <footer>{action}</footer>
            </main>
          )}
        </AboutAccess>
      </I18nProvider>
    </MixRelayProvider>,
  );
  const trigger = screen.getByRole('button', { name: 'About MixRelay' });
  expect(trigger).toHaveTextContent('0.1.0');
  expect(trigger).toHaveAttribute('aria-haspopup', 'dialog');
  trigger.focus();
  fireEvent.click(trigger);
  const dialog = await screen.findByRole('dialog', { name: 'About MixRelay' });
  expect(within(dialog).getByText('rawsector')).toBeVisible();
  const terms = within(dialog).getByText('Source licence');
  const disclosure = terms.closest('details');
  expect(disclosure).not.toHaveAttribute('open');
  expect(disclosure?.querySelector('pre')).toHaveAttribute('lang', 'en');
  expect(disclosure?.querySelector('pre')?.textContent).toBe(sourceLicense);
  fireEvent.click(terms);
  expect(disclosure).toHaveAttribute('open');
  expect(disclosure?.querySelector('pre')).toBeVisible();
  expect(within(dialog).getByText('IBM Plex · SIL Open Font License 1.1')).toBeVisible();
  fireEvent.click(within(dialog).getByRole('button', { name: 'Close' }));
  await waitFor(() => expect(trigger).toHaveFocus());
  expect(screen.getByText('Workspace')).toBeVisible();
});
