import type { DriverSnapshot } from '../../core/domain/driver';

export const readyDriver: DriverSnapshot = {
  protocolVersion: 1,
  revision: 7,
  state: 'ready',
  installedVersion: '0.1.0',
  settings: {
    canCreate: true,
    canRename: true,
    renderChannels: [1, 2, 4],
    captureChannels: [1, 2, 4],
    creationTemplate: {
      render: { channelLayout: 'stereo', channels: 2 },
      capture: { channelLayout: 'stereo', channels: 2 },
      supportedSampleRates: [44100, 48000],
      defaultSampleRate: 48000,
    },
    devices: [
      {
        spec: {
          id: 'device-a',
          displayName: 'Monitor',
          render: { channelLayout: 'stereo', channels: 2 },
          capture: { channelLayout: 'stereo', channels: 2 },
          supportedSampleRates: [44100, 48000],
          defaultSampleRate: 48000,
        },
        nativeDeviceIds: ['native-a'],
        observedSampleRate: 48000,
        supportedSampleRates: [44100, 48000],
        canSetSampleRate: true,
        nativeFormats: {
          render: {
            current: { sampleRate: 48000, channels: 2, sampleFormat: 'float32' },
            supported: [
              { sampleRate: 48000, channels: 2, sampleFormat: 'float32' },
              { sampleRate: 48000, channels: 2, sampleFormat: 'signed32' },
              { sampleRate: 44100, channels: 2, sampleFormat: 'signed24' },
            ],
          },
          capture: {
            current: { sampleRate: 48000, channels: 2, sampleFormat: 'signed16' },
            supported: [
              { sampleRate: 48000, channels: 2, sampleFormat: 'signed16' },
              { sampleRate: 48000, channels: 2, sampleFormat: 'float32' },
            ],
          },
        },
      },
    ],
  },
};
