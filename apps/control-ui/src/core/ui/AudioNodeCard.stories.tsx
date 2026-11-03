import type { Meta, StoryObj } from '@storybook/react-vite';

import mixrelayAppIcon from '../../../../desktop/src-tauri/icons/32x32.png';
import { AudioNodeCard } from './AudioNodeCard';

const meta = {
  title: 'Core/Audio Nodes/Application Icon',
  component: AudioNodeCard,
  args: {
    category: 'Application',
    icon: 'APP',
    iconUrl: mixrelayAppIcon,
    title: 'MixRelay',
    inputCount: 0,
    outputCount: 2,
    onSelect: () => {},
  },
  decorators: [
    (Story) => (
      <div style={{ width: 'var(--node-default-width)' }}>
        <Story />
      </div>
    ),
  ],
} satisfies Meta<typeof AudioNodeCard>;

export default meta;
type Story = StoryObj<typeof meta>;

/** Use the actual application asset, rather than drawing a replacement logo. */
export const ActualAppIcon: Story = {};
export const IconUnavailable: Story = { args: { iconUrl: undefined } };
export const IconLoadFailure: Story = { args: { iconUrl: 'data:image/png;base64,broken' } };

export const ParentOwnsSelection: Story = { args: { onSelect: undefined } };

export const DeviceApps: Story = {
  args: {
    title: 'MixRelay Virtual I/O',
    category: 'Device',
    icon: 'ADC',
    iconUrl: undefined,
    deviceAppActivity: {
      direction: 'playing',
      summary: 'Apps playing to MixRelay Virtual I/O: MixRelay, Safari, Music, Firefox',
      label: 'No apps',
      apps: [
        { id: 'mixrelay', label: 'MixRelay — playing to device', iconUrl: mixrelayAppIcon },
        { id: 'safari', label: 'Safari — playing to device' },
        { id: 'music', label: 'Music — playing to device' },
        { id: 'firefox', label: 'Firefox — playing to device' },
      ],
      overflowLabel: '+1',
      overflowDescription: 'Additional apps playing to device (1): Firefox',
    },
  },
};
export const DeviceAppsMinimumWidth: Story = {
  ...DeviceApps,
  decorators: [
    (Story) => (
      <div style={{ width: 180 }}>
        <Story />
      </div>
    ),
  ],
};
