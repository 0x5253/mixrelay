import { expect, it, vi } from 'vitest';
import { TauriRuntimePort } from './TauriRuntimePort';

function port() {
  const invoke = vi.fn();
  let deliver: (message: unknown) => void = () => undefined;
  const channel = vi.fn((onMessage: (message: unknown) => void) => {
    deliver = onMessage;
    return { channel: 'stages' };
  });
  return {
    invoke,
    channel,
    deliver: (message: unknown) => deliver(message),
    port: new TauriRuntimePort(invoke, channel),
  };
}

it('observes runtime status without restarting it and connects only on an explicit request', async () => {
  const { invoke, port: runtime } = port();
  invoke.mockResolvedValueOnce('unavailable').mockResolvedValueOnce('ready');
  await expect(runtime.status()).resolves.toBe('unavailable');
  expect(invoke).toHaveBeenCalledExactlyOnceWith('runtime_status');
  await expect(runtime.connect(vi.fn())).resolves.toBe('ready');
  expect(invoke).toHaveBeenLastCalledWith('runtime_connect', { onStage: { channel: 'stages' } });
});

it('forwards known connection steps and ignores anything else on the channel', async () => {
  const { invoke, deliver, port: runtime } = port();
  invoke.mockResolvedValueOnce('ready');
  const onStage = vi.fn();
  await runtime.connect(onStage);
  for (const message of ['starting', 'restarting', 3, null, 'preparing_engine']) deliver(message);
  expect(onStage.mock.calls).toEqual([['starting'], ['preparing_engine']]);
});

it('refuses an outcome that is not a runtime status and preserves native failures', async () => {
  const { invoke, port: runtime } = port();
  invoke.mockResolvedValueOnce('starting').mockResolvedValueOnce({ status: 'ready' });
  await expect(runtime.status()).rejects.toThrow('invalid response at runtime_status');
  await expect(runtime.connect(vi.fn())).rejects.toThrow('runtime_connect');
  const failure = new Error('Driver is not installed');
  invoke.mockRejectedValueOnce(failure);
  await expect(runtime.connect(vi.fn())).rejects.toBe(failure);
});
