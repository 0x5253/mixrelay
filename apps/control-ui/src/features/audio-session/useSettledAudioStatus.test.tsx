import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { MixRelayStatus } from '../../core/domain/control/status';
import { feedback } from '../../core/theme/tokens';
import { useSettledAudioStatus } from './useSettledAudioStatus';

const healthy: MixRelayStatus = {
  protocolVersion: 1,
  driverState: 'ready',
  sampleRate: 48000,
  bufferFrames: 128,
  underruns: 0,
  overruns: 0,
  audioSession: {
    format: { sampleRate: 48000, blockFrames: 128 },
    clockAuthority: 'engine_timer',
    observedDeviceSampleRate: null,
    transportLatencyFrames: null,
    processingState: 'running',
    issue: null,
    capabilities: {
      backend: 'core_audio',
      supportedSampleRates: [48000],
      supportedBlockFrames: [128],
      canReconfigureLive: true,
      hardwareInput: true,
      hardwareOutput: true,
      virtualEndpoint: true,
      applicationCapture: true,
      sampleRateConversion: true,
      driftCorrection: true,
    },
  },
};
const unsynchronized: MixRelayStatus = {
  ...healthy,
  underruns: 3,
  audioSession: {
    ...(healthy.audioSession as NonNullable<MixRelayStatus['audioSession']>),
    issue: 'clock_not_synchronized',
  },
};

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

function render() {
  return renderHook(({ status }) => useSettledAudioStatus(status), {
    initialProps: { status: healthy as MixRelayStatus | null },
  });
}

it('shows live readings without a sub-second issue, then the issue once it lasts', () => {
  const view = render();
  view.rerender({ status: unsynchronized });
  expect(view.result.current?.underruns).toBe(3);
  expect(view.result.current?.audioSession?.issue).toBeNull();
  act(() => vi.advanceTimersByTime(feedback.problemDelay));
  expect(view.result.current).toBe(unsynchronized);
});

it('bridges one missed status reply with the last observation', () => {
  const view = render();
  view.rerender({ status: null });
  expect(view.result.current).toBe(healthy);
  act(() => vi.advanceTimersByTime(feedback.problemDelay));
  expect(view.result.current).toBeNull();
});

it('shows a processing failure at once', () => {
  const view = render();
  const failed: MixRelayStatus = {
    ...healthy,
    audioSession: {
      ...(healthy.audioSession as NonNullable<MixRelayStatus['audioSession']>),
      issue: 'processing_failed',
    },
  };
  view.rerender({ status: failed });
  expect(view.result.current).toBe(failed);
});
