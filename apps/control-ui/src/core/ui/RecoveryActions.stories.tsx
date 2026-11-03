import type { Meta, StoryObj } from '@storybook/react-vite';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ActionButton } from './ActionButton';
import { InlineNotice } from './InlineNotice';
import { RecoveryActions } from './RecoveryActions';

function RecoveryExample() {
  const { t } = useTranslation();
  const [blocked, setBlocked] = useState(true);
  return blocked ? (
    <InlineNotice
      tone="critical"
      actions={
        <RecoveryActions
          retryLabel={t('routing.recovery.retry')}
          reloadLabel={t('routing.recovery.reload')}
          onRetry={() => setBlocked(false)}
          onReload={() => setBlocked(false)}
        />
      }
    >
      {t('routing.recovery.draftPreserved')}
    </InlineNotice>
  ) : (
    <ActionButton onClick={() => setBlocked(true)}>{t('routing.recovery.retry')}</ActionButton>
  );
}

const meta = { title: 'Core/Feedback/Recovery Actions', component: RecoveryExample } satisfies Meta<
  typeof RecoveryExample
>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Blocked: Story = {};
export const NarrowLongTranslation: Story = {
  globals: { locale: 'pseudo', theme: 'dark' },
  decorators: [
    (Story) => (
      <div style={{ width: 280 }}>
        <Story />
      </div>
    ),
  ],
};
