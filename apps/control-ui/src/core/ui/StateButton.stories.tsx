import type { Meta, StoryObj } from '@storybook/react-vite';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { StateButton } from './StateButton';

function PlaybackButton({
  pending = false,
  failed = false,
}: {
  pending?: boolean;
  failed?: boolean;
}) {
  const { t } = useTranslation();
  const [paused, setPaused] = useState(false);
  return (
    <StateButton
      states={[
        { value: 'pause', label: t('audioSession.pause'), icon: 'Ⅱ' },
        { value: 'resume', label: t('audioSession.resume'), icon: '▷' },
        { value: 'pending', label: t('audioSession.changing'), icon: 'Ⅱ' },
      ]}
      value={pending ? 'pending' : paused ? 'resume' : 'pause'}
      tone={failed ? 'critical' : pending ? 'warning' : paused ? 'neutral' : 'positive'}
      aria-pressed={paused}
      aria-busy={pending}
      disabled={pending}
      onClick={() => setPaused((value) => !value)}
    />
  );
}

const meta = {
  title: 'Core/Controls/StateButton',
  component: PlaybackButton,
} satisfies Meta<typeof PlaybackButton>;
export default meta;
type Story = StoryObj<typeof meta>;

export const Interactive: Story = {};
export const Pending: Story = { args: { pending: true } };
export const Failed: Story = { args: { failed: true } };
export const NarrowLongTranslation: Story = {
  globals: { locale: 'pseudo' },
  decorators: [
    (Story) => (
      <div style={{ width: 320 }}>
        <Story />
      </div>
    ),
  ],
};
