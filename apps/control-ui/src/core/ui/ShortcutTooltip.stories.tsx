import type { Meta, StoryObj } from '@storybook/react-vite';

import { ActionButton } from './ActionButton';
import { ShortcutTooltip } from './ShortcutTooltip';

const meta = {
  title: 'Core/Feedback/ShortcutTooltip',
  component: ShortcutTooltip,
  args: {
    children: <ActionButton>New graph</ActionButton>,
    shortcut: 'Mod+N',
  },
} satisfies Meta<typeof ShortcutTooltip>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {};
