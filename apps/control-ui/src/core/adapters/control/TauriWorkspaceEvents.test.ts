import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { expect, it, vi } from 'vitest';
import { protocolFixtures } from '../../../test-support/protocolFixtures';
import { TauriControlPort } from './TauriControlPort';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }));

it('forwards only recognized native file actions and returns the listener cleanup', async () => {
  const cleanup = vi.fn();
  vi.mocked(listen).mockResolvedValueOnce(cleanup);
  const listener = vi.fn();
  await expect(new TauriControlPort().onWorkspaceFile(listener)).resolves.toBe(cleanup);
  const registered = vi.mocked(listen).mock.calls[0];
  if (!registered) throw new Error('Workspace event listener was not registered');
  expect(registered[0]).toBe('workspace-file');
  const callback = registered[1];
  for (const payload of ['new', 'open', 'save', 'save_as', 'remove', '']) {
    callback({ event: 'workspace-file', id: 1, payload });
  }
  expect(listener.mock.calls).toEqual([['new'], ['open'], ['save'], ['save_as']]);
  expect(invoke).not.toHaveBeenCalled();
});

it('the packaged adapter uses the real Tauri invocation boundary by default', async () => {
  vi.mocked(invoke).mockResolvedValueOnce({ status: 200, body: protocolFixtures.status() });
  await expect(new TauriControlPort().getStatus()).resolves.toMatchObject({ driverState: 'ready' });
  expect(invoke).toHaveBeenCalledExactlyOnceWith('control_get_status', undefined);
});
