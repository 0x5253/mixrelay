import type { Meta, StoryObj } from '@storybook/react-vite';

import { Meter } from './Meter';

const meta = {
  title: 'Core/Feedback/Meter',
  component: Meter,
  args: { label: 'Input level', level: 65 },
  decorators: [
    (Story) => (
      <div style={{ height: 120, width: 200 }}>
        <Story />
      </div>
    ),
  ],
} satisfies Meta<typeof Meter>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Vertical: Story = {};
export const Horizontal: Story = { args: { orientation: 'horizontal' } };
export const Silent: Story = { args: { level: 0 } };
export const High: Story = { args: { level: 85 } };
export const Peak: Story = { args: { level: 100 } };
