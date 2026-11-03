import { afterEach, describe, expect, it, vi } from 'vitest';
import { monotonicClock, timerScheduler } from '../../core/adapters/browser/timers';
import { TauriControlPort } from '../../core/adapters/control/TauriControlPort';
import type { MixRelayStatus } from '../../core/domain/control/status';

import { waitForEnginePower } from './application/waitForEnginePower';

const status: MixRelayStatus = {
  protocolVersion: 1,
  driverState: 'ready',
  sampleRate: 48000,
  bufferFrames: 128,
  underruns: 0,
  overruns: 0,
};
function fixture() {
  const port = new TauriControlPort();
  const getStatus = vi.spyOn(port, 'getStatus');
  return { port, getStatus };
}

afterEach(() => vi.useRealTimers());

describe('native pause and resume acknowledgement', () => {
  it.each([true, false])(
    'accepts acknowledged enabled=%s only after transition has finished',
    async (enabled) => {
      vi.useFakeTimers();
      const { port, getStatus } = fixture();
      const ready = { ...status, enginePower: { enabled, transitioning: false } };
      getStatus
        .mockResolvedValueOnce(status)
        .mockResolvedValueOnce({
          ...status,
          enginePower: { enabled: !enabled, transitioning: false },
        })
        .mockResolvedValueOnce({ ...status, enginePower: { enabled, transitioning: true } })
        .mockResolvedValueOnce(ready);
      const result = waitForEnginePower(port, enabled, () => true, timerScheduler, monotonicClock);
      await vi.advanceTimersByTimeAsync(300);
      await expect(result).resolves.toBe(ready);
      expect(getStatus).toHaveBeenCalledTimes(4);
      expect(vi.getTimerCount()).toBe(0);
    },
  );

  it('does not poll if the request owner was already replaced', async () => {
    const { port, getStatus } = fixture();
    await expect(
      waitForEnginePower(port, false, () => false, timerScheduler, monotonicClock),
    ).resolves.toBeNull();
    expect(getStatus).not.toHaveBeenCalled();
  });

  it('ignores an acknowledgement that arrives after the request owner was replaced', async () => {
    const { port, getStatus } = fixture();
    let current = true;
    getStatus.mockImplementationOnce(async () => {
      current = false;
      return { ...status, enginePower: { enabled: false, transitioning: false } };
    });
    await expect(
      waitForEnginePower(port, false, () => current, timerScheduler, monotonicClock),
    ).resolves.toBeNull();
    expect(getStatus).toHaveBeenCalledOnce();
  });

  it('stops at the acknowledgement deadline without leaving a timer or retrying the write', async () => {
    vi.useFakeTimers();
    const { port, getStatus } = fixture();
    getStatus.mockResolvedValue(status);
    const result = waitForEnginePower(
      port,
      false,
      () => true,
      timerScheduler,
      monotonicClock,
    ).catch((error: unknown) => error);
    await vi.advanceTimersByTimeAsync(10000);
    expect(await result).toEqual(new Error('Engine power acknowledgement timed out'));
    expect(getStatus).toHaveBeenCalledTimes(100);
    expect(vi.getTimerCount()).toBe(0);
  });

  it('preserves a native control failure for the caller to show without replacing it with a timeout', async () => {
    const { port, getStatus } = fixture();
    const error = new Error('Driver disconnected');
    getStatus.mockRejectedValueOnce(error);
    await expect(
      waitForEnginePower(port, true, () => true, timerScheduler, monotonicClock),
    ).rejects.toBe(error);
  });
});
