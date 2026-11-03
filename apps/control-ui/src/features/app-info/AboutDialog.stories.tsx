import type { Meta, StoryObj } from '@storybook/react-vite';
import { fn } from 'storybook/test';
import { AboutDialog } from './AboutDialog';

const meta = {
  title: 'Features/Application/Product Information',
  component: AboutDialog,
  args: { version: '0.1.3', opened: true, onClose: fn() },
} satisfies Meta<typeof AboutDialog>;
export default meta;
type Story = StoryObj<typeof meta>;
export const About: Story = {};
export const LongTranslation: Story = { globals: { locale: 'pseudo' } };
