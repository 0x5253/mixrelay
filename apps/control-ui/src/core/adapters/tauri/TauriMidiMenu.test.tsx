import { act } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { TauriMidiPort } from './TauriMidiPort';

const native = vi.hoisted(() => ({ listen: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: native.listen }));
afterEach(() => {
  vi.resetAllMocks();
  vi.restoreAllMocks();
});
it('binds the native Cmd/Ctrl+M menu event and releases it without late callbacks', async () => {
  const stop = vi.fn();
  native.listen.mockResolvedValue(stop);
  const listener = vi.fn();
  const port = new TauriMidiPort();
  const close = port.subscribeMapToggle(listener);
  await act(async () => {});
  expect(native.listen).toHaveBeenCalledWith('midi-map-toggle', expect.any(Function));
  native.listen.mock.calls[0]?.[1]({ payload: null });
  expect(listener).toHaveBeenCalledOnce();
  close();
  expect(stop).toHaveBeenCalledOnce();
  native.listen.mock.calls[0]?.[1]({ payload: null });
  expect(listener).toHaveBeenCalledOnce();
});
it('cleans up registration that completes after its editor owner closes', async () => {
  const pending = Promise.withResolvers<() => void>();
  native.listen.mockReturnValue(pending.promise);
  const close = new TauriMidiPort().subscribeMapToggle(vi.fn());
  close();
  const stop = vi.fn();
  await act(async () => pending.resolve(stop));
  expect(stop).toHaveBeenCalledOnce();
});
it('reports event bus registration failure without throwing or leaking a subscription', async () => {
  const log = vi.spyOn(console, 'error').mockImplementation(() => {});
  native.listen.mockRejectedValue(new Error('event bus unavailable'));
  const close = new TauriMidiPort().subscribeMapToggle(vi.fn());
  await act(async () => {});
  expect(log).toHaveBeenCalledWith('MIDI menu listener failed', expect.any(Error));
  close();
});
