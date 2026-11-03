import type { Meta, StoryObj } from '@storybook/react-vite';

import { DriverStatusCard } from './DriverStatusCard';

const meta = {
  title: 'Features/Devices/DriverStatusCard',
  component: DriverStatusCard,
  args: { state: 'ready' },
} satisfies Meta<typeof DriverStatusCard>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Ready: Story = {};
export const NotInstalled: Story = { args: { state: 'not_installed' } };
export const Loading: Story = { args: { state: 'installing' } };
export const DriverRestartRequired: Story = { args: { state: 'restart_required' } };
export const InstalledNotLoaded: Story = { args: { state: 'installed_not_loaded' } };
export const PermissionDenied: Story = { args: { state: 'invalid_permissions' } };
export const VersionMismatch: Story = { args: { state: 'version_mismatch' } };
export const Blocked: Story = { args: { state: 'blocked' } };
export const Unavailable: Story = { args: { state: 'unavailable' } };
export const LongTranslation: Story = {
  args: { state: 'restart_required' },
  globals: { locale: 'pseudo' },
};
export const MobileNarrow: Story = {
  args: { state: 'not_installed' },
  parameters: { viewport: { defaultViewport: 'mobile' } },
};
