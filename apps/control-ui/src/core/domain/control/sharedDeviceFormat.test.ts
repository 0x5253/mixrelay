import { expect, it } from 'vitest';
import { sharedChannelCounts, sharedDevicePcm } from './sharedDeviceFormat';

it('intersects PCM choices at each native direction’s actual rate and channel count', () => {
  const render = {
    current: { sampleRate: 48000, channels: 2, sampleFormat: 'float32' as const },
    supported: [
      { sampleRate: 48000, channels: 2, sampleFormat: 'float32' as const },
      { sampleRate: 48000, channels: 2, sampleFormat: 'signed32' as const },
      { sampleRate: 44100, channels: 2, sampleFormat: 'signed24' as const },
    ],
  };
  const capture = {
    current: { sampleRate: 44100, channels: 4, sampleFormat: 'signed16' as const },
    supported: [
      { sampleRate: 44100, channels: 4, sampleFormat: 'float32' as const },
      { sampleRate: 44100, channels: 4, sampleFormat: 'signed16' as const },
    ],
  };
  expect(sharedDevicePcm({ render, capture }, true, true)).toEqual({
    value: null,
    mixed: true,
    options: ['float32'],
  });
  expect(
    sharedDevicePcm({ render, capture: { current: null, supported: [] } }, true, false),
  ).toEqual({ value: 'float32', mixed: false, options: ['signed32', 'float32'] });
});
it('offers only common channel counts and preserves a single-direction capability', () => {
  expect(sharedChannelCounts([1, 2, 4, 64], [2, 4, 16])).toEqual([2, 4]);
  expect(sharedChannelCounts([], [2, 4], false, true)).toEqual([2, 4]);
  expect(sharedChannelCounts([2, 4], [], true, false)).toEqual([2, 4]);
  expect(sharedChannelCounts([], [], false, false)).toEqual([]);
});

it('offers no shared PCM command when a direction has no native current format', () => {
  const unavailable = { current: null, supported: [] };
  expect(sharedDevicePcm({ render: unavailable, capture: unavailable }, true, true)).toEqual({
    value: null,
    options: [],
    mixed: false,
  });
  expect(sharedDevicePcm({ render: unavailable, capture: unavailable }, false, false)).toEqual({
    value: null,
    options: [],
    mixed: false,
  });
});
