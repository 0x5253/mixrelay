import { fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { i18n } from '../../core/i18n';
import { MixRelayProvider } from '../../core/theme/MixRelayProvider';
import { DriverInstallNotice } from './DriverInstallNotice';

it('explains missing virtual devices without offering an unavailable installer action', async () => {
  await i18n.changeLanguage('en');
  const view = render(
    <MixRelayProvider>
      <DriverInstallNotice state="not_installed" className="workspace-notice" />
    </MixRelayProvider>,
  );
  expect(screen.getByRole('complementary', { name: 'Virtual audio driver' })).toHaveClass(
    'workspace-notice',
  );
  expect(screen.getByText(i18n.t('driver.installNotice'))).toBeInTheDocument();
  expect(screen.queryByRole('button')).not.toBeInTheDocument();
  view.rerender(<DriverInstallNotice state="ready" />);
  expect(screen.queryByRole('complementary')).not.toBeInTheDocument();
});

it('hands the installer request to its owner only when the driver is missing', async () => {
  await i18n.changeLanguage('en');
  const onInstall = vi.fn();
  const view = render(
    <MixRelayProvider>
      <DriverInstallNotice state="not_installed" onInstall={onInstall} />
    </MixRelayProvider>,
  );
  fireEvent.click(screen.getByRole('button', { name: i18n.t('driver.getInstaller') }));
  expect(onInstall).toHaveBeenCalledOnce();
  view.rerender(<DriverInstallNotice onInstall={onInstall} />);
  expect(screen.queryByRole('button')).not.toBeInTheDocument();
});
