import { fireEvent, render, screen } from '@testing-library/react';
import { I18nextProvider } from 'react-i18next';
import { describe, expect, it, vi } from 'vitest';
import type { DriverState } from '../../core/domain/control/status';

import { i18n } from '../../core/i18n';
import { MixRelayProvider } from '../../core/theme/MixRelayProvider';
import { DriverStatusCard } from './DriverStatusCard';

describe('DriverStatusCard', () => {
  it('maps a stable machine state to localized UI and action', async () => {
    await i18n.changeLanguage('en');
    const onAction = vi.fn();

    render(
      <MixRelayProvider>
        <I18nextProvider i18n={i18n}>
          <DriverStatusCard onAction={onAction} state="restart_required" />
        </I18nextProvider>
      </MixRelayProvider>,
    );

    expect(screen.getByText('Restart required')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Restart audio services' }));
    expect(onAction).toHaveBeenCalledOnce();
  });

  it('renders every stable driver state and disables actions only while busy', async () => {
    await i18n.changeLanguage('en');
    const states: DriverState[] = [
      'not_installed',
      'installing',
      'restart_required',
      'installed_not_loaded',
      'blocked',
      'unavailable',
      'invalid_permissions',
      'version_mismatch',
      'ready',
    ];

    for (const state of states) {
      const view = render(
        <MixRelayProvider>
          <I18nextProvider i18n={i18n}>
            <DriverStatusCard state={state} onAction={vi.fn()} />
          </I18nextProvider>
        </MixRelayProvider>,
      );
      const button = screen.getByRole('button');
      expect(button).toHaveProperty('disabled', state === 'installing');
      view.unmount();
    }
  });
});

it('keeps a read-only diagnostic visible without inventing an actionable command', async () => {
  await i18n.changeLanguage('en');
  render(
    <MixRelayProvider>
      <DriverStatusCard state="ready">
        <p>Device owner connected</p>
      </DriverStatusCard>
    </MixRelayProvider>,
  );
  expect(screen.getByText('Device owner connected')).toBeInTheDocument();
  expect(screen.getByText(i18n.t('driver.states.ready'))).toBeInTheDocument();
  expect(screen.queryByRole('button')).not.toBeInTheDocument();
});
