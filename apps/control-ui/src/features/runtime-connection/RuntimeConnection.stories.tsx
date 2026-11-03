import type { Meta, StoryObj } from '@storybook/react-vite';
import { RuntimeConnection } from './RuntimeConnection';

const meta = {
  title: 'Features/Startup/DriverConnection',
  component: RuntimeConnection,
  parameters: { layout: 'fullscreen' },
  args: {
    port: { connect: () => new Promise(() => {}), status: async () => 'ready' as const },
    onInstall: () => {},
    children: <p>Workspace ready</p>,
  },
} satisfies Meta<typeof RuntimeConnection>;
export default meta;
type Story = StoryObj<typeof meta>;

export const Connecting: Story = {};
export const Starting: Story = {
  args: {
    port: {
      connect: (onStage) => {
        onStage('starting');
        return new Promise(() => {});
      },
      status: async () => 'ready' as const,
    },
  },
};
export const PreparingEngine: Story = {
  args: {
    port: {
      connect: (onStage) => {
        onStage('preparing_engine');
        return new Promise(() => {});
      },
      status: async () => 'ready' as const,
    },
  },
};
export const ConnectingLongTranslation: Story = {
  ...PreparingEngine,
  globals: { locale: 'pseudo' },
};
export const ConnectingNarrow: Story = {
  ...PreparingEngine,
  parameters: { viewport: { defaultViewport: 'mobile' } },
};
export const NotInstalled: Story = {
  args: { port: { connect: async () => 'not_installed', status: async () => 'not_installed' } },
};
export const Incompatible: Story = {
  args: { port: { connect: async () => 'incompatible', status: async () => 'incompatible' } },
};
export const Unavailable: Story = {
  args: { port: { connect: async () => 'unavailable', status: async () => 'unavailable' } },
};
export const StartupRecovery: Story = {
  args: { port: { connect: async () => 'unavailable', status: async () => 'ready' } },
};
export const LongTranslation: Story = { ...NotInstalled, globals: { locale: 'pseudo' } };
export const Narrow: Story = {
  ...NotInstalled,
  parameters: { viewport: { defaultViewport: 'mobile' } },
};
