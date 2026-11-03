import type { Meta, StoryObj } from '@storybook/react-vite';

import { StatusBadge } from './StatusBadge';

const meta = {
  title: 'Core/Feedback/StatusBadge',
  component: StatusBadge,
  args: { children: 'Ready', tone: 'neutral' },
  argTypes: {
    children: { control: 'text', description: '사용자가 읽을 수 있는 현재 상태.' },
    tone: { control: 'select', options: ['neutral', 'accent', 'positive', 'warning', 'critical'] },
  },
  parameters: {
    controls: { include: ['children', 'tone'] },
    docs: { source: { type: 'dynamic' } },
  },
} satisfies Meta<typeof StatusBadge>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {};
export const Playground: Story = {};
export const Information: Story = { args: { tone: 'accent', children: 'Update available' } };
export const Positive: Story = { args: { tone: 'positive' } };
export const Warning: Story = { args: { tone: 'warning', children: 'Restart required' } };
export const Critical: Story = { args: { tone: 'critical', children: 'Driver blocked' } };
