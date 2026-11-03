import type { Meta, StoryObj } from '@storybook/react-vite';
import type { ComponentProps } from 'react';
import {
  type StorybookControlState,
  StorybookMockControlPort,
} from '../core/adapters/fake/StorybookMockControlPort';
import type { RuntimeGraphDocument } from '../core/domain/control/graph';

import { StudioScreen } from './StudioScreen';

const graph: RuntimeGraphDocument = {
  edges: [],
  nodes: [
    {
      config: {
        id: 'b22d9d37-d558-4a0c-a77a-d1b79bc5032b',
        displayName: 'MixRelay IO',
        supportedSampleRates: [48_000],
        reference: { render: true, capture: true },
        capture: { channelLayout: 'stereo', channels: 2 },
        defaultSampleRate: 48_000,
        render: { channelLayout: 'stereo', channels: 2 },
      },
      id: 'default-virtual-io',
      label: 'MixRelay Virtual I/O',
      type: 'virtual_endpoint',
    },
  ],
  revision: 1,
  schemaVersion: 2,
};

const controlState = {
  applications: [
    {
      bundleId: 'org.mozilla.firefox',
      displayName: 'Firefox',
      iconDataUrl: null,
      processId: 1001,
      selector: {
        match: 'bundle_id',
        bundle_id: 'org.mozilla.firefox',
        include_descendants: false,
      },
    },
    {
      bundleId: 'com.google.Chrome',
      displayName: 'Google Chrome',
      iconDataUrl: null,
      processId: 1002,
      selector: { match: 'bundle_id', bundle_id: 'com.google.Chrome', include_descendants: false },
    },
    {
      bundleId: 'com.obsproject.obs-studio',
      displayName: 'OBS',
      iconDataUrl: null,
      processId: 1003,
      selector: {
        match: 'bundle_id',
        bundle_id: 'com.obsproject.obs-studio',
        include_descendants: false,
      },
    },
  ],
  audioDevices: [
    {
      displayName: 'MacBook Pro Microphone',
      inputChannels: 2,
      isDefaultInput: true,
      isDefaultOutput: false,
      outputChannels: 0,
      uid: 'BuiltInMicrophoneDevice',
    },
    {
      displayName: 'MacBook Pro Speakers',
      inputChannels: 0,
      isDefaultInput: false,
      isDefaultOutput: true,
      outputChannels: 2,
      uid: 'BuiltInSpeakerDevice',
    },
    {
      displayName: 'MixRelay Virtual I/O',
      inputChannels: 2,
      isDefaultInput: false,
      isDefaultOutput: false,
      outputChannels: 2,
      uid: 'io.mixrelay.virtual-io',
    },
  ],
  graph,
  graphStatus: { activeRevision: 1, protocolVersion: 1, submittedRevision: 1 },
  meters: {
    nodes: [{ nodeId: 'default-virtual-io', peaks: [0.42, 0.19], portId: 'from_os' }],
    protocolVersion: 1,
    revision: 1,
  },
  status: {
    bufferFrames: 128,
    driverState: 'ready',
    overruns: 0,
    protocolVersion: 1,
    sampleRate: 48_000,
    underruns: 0,
  },
} satisfies StorybookControlState;

const meta = {
  title: 'Screens/Studio',
  component: StudioScreen,
  loaders: [() => ({ controlPort: new StorybookMockControlPort(structuredClone(controlState)) })],
  render: (args, { loaded }) => <StudioScreen {...args} controlPort={loaded.controlPort} />,
  parameters: { layout: 'fullscreen' },
} satisfies Meta<typeof StudioScreen>;

export default meta;
type Story = StoryObj<Partial<ComponentProps<typeof StudioScreen>>>;

export const Default: Story = {};
export const MobileNarrow: Story = {
  globals: { viewport: { value: 'mobile' } },
};
export const LongTranslation: Story = { globals: { locale: 'pseudo' } };
