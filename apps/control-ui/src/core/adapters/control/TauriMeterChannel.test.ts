import { expect, it, vi } from 'vitest';
import { TauriControlPort } from './TauriControlPort';

const native = vi.hoisted(() => ({
  invoke: vi.fn().mockResolvedValue(3),
  Channel: class {
    onmessage: (message: unknown) => void = () => undefined;
  },
}));
vi.mock('@tauri-apps/api/core', () => native);

it('creates a native channel for pushed meters by default', async () => {
  const port = new TauriControlPort();
  await port.subscribeMeters({ window: vi.fn(), ended: vi.fn() });
  expect(native.invoke).toHaveBeenCalledWith('control_subscribe_telemetry', {
    channel: expect.any(native.Channel),
  });
});
