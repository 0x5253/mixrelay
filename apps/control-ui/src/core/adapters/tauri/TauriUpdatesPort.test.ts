import { expect, it, vi } from 'vitest';
import { TauriUpdatesPort } from './TauriUpdatesPort';

const update = {
  product: 'mix-relay',
  installedVersion: '0.1.0',
  latestVersion: '0.2.0',
  state: 'available',
  downloads: [{ os: 'macos', arch: 'universal', url: 'https://x/y.pkg', sha256: 'a', size: 9 }],
  notesUrl: null,
};

it('checks and opens an installer through native catalog commands', async () => {
  const invoke = vi.fn().mockResolvedValueOnce([update]).mockResolvedValueOnce(undefined);
  const port = new TauriUpdatesPort(invoke);
  await expect(port.check()).resolves.toEqual([update]);
  await port.openDownload('mix-relay', 'https://downloads.example/driver.pkg');
  expect(invoke.mock.calls).toEqual([
    ['updates_check'],
    [
      'updates_open_download',
      { product: 'mix-relay', url: 'https://downloads.example/driver.pkg' },
    ],
  ]);
});

it('refuses a catalog with an unknown product or publication state', async () => {
  const invoke = vi.fn();
  const port = new TauriUpdatesPort(invoke);
  for (const reply of [{}, [{ ...update, product: 'driver' }], [{ ...update, state: 'beta' }]]) {
    invoke.mockResolvedValueOnce(reply);
    await expect(port.check()).rejects.toThrow('invalid response at updates_check');
  }
});
