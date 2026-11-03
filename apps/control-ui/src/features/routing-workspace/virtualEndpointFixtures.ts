import type { NativeDeviceFormats } from '../../core/domain/control/nativeFormat';
import type {
  VirtualEndpointSettings,
  VirtualEndpointSpec,
} from '../../core/domain/control/status';

export const unknownNativeFormats: NativeDeviceFormats = {
  render: { current: null, supported: [] },
  capture: { current: null, supported: [] },
};

export const virtualSpec: VirtualEndpointSpec = {
  id: '095af572-6846-4ce6-b498-6dbeddb07435',
  displayName: 'SPAT send-return',
  render: { channels: 64, channelLayout: 'discrete' },
  capture: { channels: 16, channelLayout: 'discrete' },
  defaultSampleRate: 44100,
  supportedSampleRates: [44100, 48000],
};

export const virtualPolicy: VirtualEndpointSettings = {
  canCreate: true,
  canRename: true,
  renderChannels: [1, 2, 8, 16, 32, 64],
  captureChannels: [1, 2, 8, 16, 32, 64],
  creationTemplate: {
    render: virtualSpec.render,
    capture: virtualSpec.capture,
    defaultSampleRate: virtualSpec.defaultSampleRate,
    supportedSampleRates: virtualSpec.supportedSampleRates,
  },
  devices: [
    {
      nativeDeviceIds: [],
      nativeFormats: unknownNativeFormats,
      spec: virtualSpec,
      observedSampleRate: 48000,
      supportedSampleRates: [44100, 48000],
      canSetSampleRate: true,
    },
  ],
};

export const duplexReference = { render: true, capture: true };

/** Native UID intentionally differs from the graph endpoint ID. */
export const deviceActivitySession: import('../../core/domain/control/status').AudioSessionStatus =
  {
    format: { sampleRate: 48000, blockFrames: 128 },
    clockAuthority: 'device',
    observedDeviceSampleRate: 48000,
    issue: null,
    transportLatencyFrames: 128,
    capabilities: {
      backend: 'core_audio',
      supportedSampleRates: [48000],
      supportedBlockFrames: [128],
      canReconfigureLive: true,
      hardwareInput: true,
      hardwareOutput: true,
      virtualEndpoint: true,
      applicationCapture: true,
      sampleRateConversion: false,
      driftCorrection: false,
    },
    virtualEndpointSettings: {
      ...virtualPolicy,
      devices: [
        {
          nativeDeviceIds: ['native-mixrelay-device'],
          nativeFormats: unknownNativeFormats,
          spec: {
            ...virtualSpec,
            id: 'default-virtual-io',
            displayName: 'MixRelay Virtual I/O',
            render: { channels: 2, channelLayout: 'stereo' },
            capture: { channels: 2, channelLayout: 'stereo' },
          },
          observedSampleRate: 48000,
          supportedSampleRates: [48000],
          canSetSampleRate: false,
        },
      ],
    },
  };

export const deviceActivityFixture: Record<
  string,
  import('../../core/domain/control/inventory').DeviceAppActivity[]
> = {
  'native-mixrelay-device': [
    {
      bundleId: 'com.spotify.client',
      displayName: 'Spotify',
      processId: 10,
      playing: true,
      recording: false,
    },
    {
      bundleId: 'com.apple.Safari',
      displayName: 'Safari',
      processId: 20,
      playing: true,
      recording: false,
    },
    {
      bundleId: 'com.apple.Music',
      displayName: 'Music',
      processId: 30,
      playing: true,
      recording: false,
    },
    {
      bundleId: 'org.mozilla.firefox',
      displayName: 'Firefox',
      processId: 40,
      playing: true,
      recording: false,
    },
    {
      bundleId: 'com.apple.QuickTimePlayerX',
      displayName: 'QuickTime Player',
      processId: 50,
      playing: false,
      recording: true,
    },
    {
      bundleId: 'com.apple.QuickTimePlayerX',
      displayName: 'QuickTime Player',
      processId: 51,
      playing: true,
      recording: false,
    },
  ],
};
