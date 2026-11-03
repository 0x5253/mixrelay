import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import type { ProductUpdate } from '../../core/domain/updates';
import { I18nProvider, i18n } from '../../core/i18n';
import type { UpdatesPort } from '../../core/ports/UpdatesPort';
import { MixRelayProvider } from '../../core/theme/MixRelayProvider';
import { ActionButton } from '../../core/ui/ActionButton';
import { UpdateAccess } from './UpdateAccess';

const product: ProductUpdate = {
  product: 'mix-relay',
  installedVersion: '0.1.0',
  latestVersion: null,
  state: 'not_published',
  downloads: [],
  notesUrl: null,
};
const download = {
  os: 'macos',
  arch: 'universal',
  url: 'https://downloads.example/MixRelay.pkg',
  sha256: 'a'.repeat(64),
  size: 12,
};
const available: ProductUpdate = {
  ...product,
  state: 'available',
  latestVersion: '0.1.1',
  downloads: [download],
};
function port(value: ProductUpdate[] = [product]): UpdatesPort {
  return {
    check: vi.fn().mockResolvedValue(value),
    openDownload: vi.fn().mockResolvedValue(undefined),
  };
}
function view(native: UpdatesPort | null) {
  return render(
    <MixRelayProvider>
      <I18nProvider>
        <UpdateAccess port={native}>
          {(action, openInstaller) => (
            <>
              {action}
              <ActionButton onClick={openInstaller}>Repair MixRelay</ActionButton>
            </>
          )}
        </UpdateAccess>
      </I18nProvider>
    </MixRelayProvider>,
  );
}
async function open() {
  fireEvent.click(await screen.findByRole('button', { name: /Check for updates/ }));
  return screen.findByRole('dialog');
}
beforeEach(async () => {
  await i18n.changeLanguage('en');
});
it('provides no browser update entry or automatic dialog', async () => {
  view(null);
  await act(async () => Promise.resolve());
  expect(screen.queryByRole('button', { name: /Check for updates/ })).not.toBeInTheDocument();
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
});
it.each(['not_published', 'unavailable', 'current', 'published'] as const)(
  'renders one complete product in state %s',
  async (state) => {
    view(
      port([
        {
          ...product,
          state,
          installedVersion: state === 'published' ? null : product.installedVersion,
          latestVersion: state === 'current' ? product.installedVersion : null,
        },
      ]),
    );
    const dialog = await open();
    await waitFor(() =>
      expect(within(dialog).getByText(i18n.t(`updates.states.${state}`))).toBeVisible(),
    );
    expect(within(dialog).getAllByRole('region')).toHaveLength(1);
    expect(within(dialog).getByRole('region', { name: 'MixRelay' })).toBeVisible();
    expect(within(dialog).queryByText('MixRelay Driver')).not.toBeInTheDocument();
    expect(within(dialog).queryByText(/Latest release:/)).not.toBeInTheDocument();
  },
);
it('missing native catalog entries are unavailable and never current', async () => {
  view(port([]));
  const dialog = await open();
  expect(
    await within(dialog).findByText('Could not check this product. Try again when online.'),
  ).toBeVisible();
  expect(within(dialog).getByText('Installed: Unknown')).toBeVisible();
});
it('repair opens the same full installer and forwards the one product identity', async () => {
  const native = port([available]);
  view(native);
  fireEvent.click(screen.getByRole('button', { name: 'Repair MixRelay' }));
  const dialog = await screen.findByRole('dialog', { name: 'Updates' });
  const asset = await within(dialog).findByRole('button', {
    name: 'Open PKG · universal download',
  });
  await waitFor(() => expect(asset).toBeEnabled());
  fireEvent.click(asset);
  await waitFor(() =>
    expect(native.openDownload).toHaveBeenCalledExactlyOnceWith('mix-relay', download.url),
  );
  expect(within(dialog).getAllByRole('region')).toHaveLength(1);
  expect(within(dialog).getByText('Latest release: 0.1.1')).toBeVisible();
});
it('falls back to platform for an extensionless asset', async () => {
  const native = port([
    {
      ...available,
      downloads: [
        { ...download, url: 'https://downloads.example/latest', os: 'linux', arch: 'aarch64' },
      ],
    },
  ]);
  view(native);
  const dialog = await open();
  const asset = await within(dialog).findByRole('button', {
    name: 'Open linux · aarch64 download',
  });
  await waitFor(() => expect(asset).toBeEnabled());
  fireEvent.click(asset);
  await waitFor(() =>
    expect(native.openDownload).toHaveBeenCalledWith(
      'mix-relay',
      'https://downloads.example/latest',
    ),
  );
});
it('missing download data does not manufacture an install action', async () => {
  const native = port();
  vi.mocked(native.check).mockResolvedValue([
    { ...available, downloads: undefined } as unknown as ProductUpdate,
  ]);
  view(native);
  const dialog = await open();
  await waitFor(() => expect(within(dialog).getByText('An update is available.')).toBeVisible());
  expect(within(dialog).queryByRole('button', { name: /download/ })).not.toBeInTheDocument();
});
it('failed refresh hides stale downloads and retries without exposing native details', async () => {
  const native = port([available]);
  vi.mocked(native.check)
    .mockResolvedValueOnce([available])
    .mockRejectedValueOnce('private URL')
    .mockResolvedValue([product]);
  view(native);
  const dialog = await open();
  expect(
    await within(dialog).findByText('Could not check this product. Try again when online.'),
  ).toBeVisible();
  expect(within(dialog).queryByRole('button', { name: /download/ })).not.toBeInTheDocument();
  expect(within(dialog).queryByText('private URL')).not.toBeInTheDocument();
  fireEvent.click(within(dialog).getByRole('button', { name: 'Check again' }));
  expect(await within(dialog).findByText('No release has been published yet.')).toBeVisible();
});
it('failed opening leaves the same full installer retryable', async () => {
  const native = port([available]);
  vi.mocked(native.openDownload).mockRejectedValueOnce('private URL').mockResolvedValue(undefined);
  view(native);
  const dialog = await open();
  const asset = await within(dialog).findByRole('button', {
    name: 'Open PKG · universal download',
  });
  await waitFor(() => expect(asset).toBeEnabled());
  fireEvent.click(asset);
  expect(await within(dialog).findByRole('alert')).toHaveTextContent(
    'Could not open the download.',
  );
  await waitFor(() => expect(asset).toBeEnabled());
  fireEvent.click(asset);
  await waitFor(() => expect(native.openDownload).toHaveBeenCalledTimes(2));
  expect(within(dialog).queryByRole('alert')).not.toBeInTheDocument();
});
it('can close while checking; completion does not reopen the dialog', async () => {
  let resolve!: (value: ProductUpdate[]) => void;
  const native = port();
  vi.mocked(native.check).mockReturnValue(
    new Promise((yes) => {
      resolve = yes;
    }),
  );
  view(native);
  const dialog = await open();
  expect(within(dialog).getByRole('status')).toHaveTextContent('Checking for updates');
  fireEvent.click(within(dialog).getByRole('button', { name: 'Close' }));
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  await act(async () => resolve([product]));
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(native.check).toHaveBeenCalledTimes(1);
});
it('offers the same Korean product update action', async () => {
  await i18n.changeLanguage('ko-KR');
  view(port());
  fireEvent.click(screen.getByRole('button', { name: '업데이트 확인' }));
  const dialog = await screen.findByRole('dialog', { name: '업데이트' });
  expect(await within(dialog).findByText('아직 게시된 릴리스가 없습니다.')).toBeVisible();
});
