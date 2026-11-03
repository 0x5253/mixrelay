import type { Meta, StoryObj } from '@storybook/react-vite';

import { TransportButton } from './TransportButton';

const meta = {
  title: 'Core/Controls/TransportButton',
  component: TransportButton,
  args: { icon: 'record', label: 'Record', disabled: false, onClick: () => {} },
} satisfies Meta<typeof TransportButton>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {};
export const Active: Story = { args: { active: true } };
export const Disabled: Story = { args: { disabled: true } };
export const ActiveDisabled: Story = { args: { active: true, disabled: true } };
export const Pause: Story = { args: { icon: 'pause', label: 'Pause recording' } };
export const Stop: Story = { args: { icon: 'stop', label: 'Stop recording' } };
export const Resume: Story = { args: { icon: 'resume', label: 'Resume recording' } };
export const FooterPause: Story = { args: { icon: 'pause', label: '일시정지', compact: true } };
export const FooterResume: Story = { args: { icon: 'resume', label: '재개', compact: true } };
export const FooterPending: Story = {
  args: {
    icon: 'pause',
    label: '오디오 상태 변경 중',
    compact: true,
    busy: true,
    disabled: true,
    tone: 'warning',
  },
};

export const PauseActiveDisabled: Story = {
  args: { icon: 'pause', label: 'Recording paused · unavailable', active: true, disabled: true },
};
export const BusyEnabled: Story = {
  args: {
    busy: true,
    disabled: false,
    label: 'Pending flag only · disabled is owned by the caller',
  },
};
