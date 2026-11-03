import type { Meta, StoryObj } from '@storybook/react-vite';
import mixrelayAppIcon from '../../../../../desktop/src-tauri/icons/32x32.png';
import { DeviceAppActivity } from '../../ui/DeviceAppActivity';

const meta = {
  title: 'Core/Playgrounds/DeviceAppActivity',
  component: DeviceAppActivity,
  args: {
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
} satisfies Meta<typeof DeviceAppActivity>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Recording: Story = {
  args: {
    direction: 'recording',
    summary: 'Apps recording from device: MixRelay',
    apps: [{ id: 'mixrelay', label: 'MixRelay — recording from device', iconUrl: mixrelayAppIcon }],
  },
};
export const Empty: Story = { args: { apps: [], summary: 'No apps playing to device' } };
export const Checking: Story = {
  args: { apps: [], label: 'Checking apps…', summary: 'Checking apps…' },
};
export const Unavailable: Story = {
  args: { apps: [], label: 'App activity unavailable', summary: 'App activity unavailable' },
};
export const BrokenIcon: Story = {
  args: {
    apps: [
      { id: 'broken', label: 'App — playing to device', iconUrl: 'data:image/png;base64,broken' },
    ],
  },
};
