import type { Meta, StoryObj } from '@storybook/react-vite';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ActionButton } from './ActionButton';
import { CheckboxField } from './CheckboxField';
import { CollectionItem } from './CollectionItem';
import styles from './ComponentCatalog.module.css';
import { IconActionButton } from './IconActionButton';
import { PathField } from './PathField';
import { RetryAlert } from './RetryAlert';
import { Text } from './Text';
import { TextField } from './TextField';
import { TitleField } from './TitleField';

function SystemControls({
  disabled = false,
  busy = false,
  selected: initiallySelected = false,
}: {
  disabled?: boolean;
  busy?: boolean;
  selected?: boolean;
}) {
  const { t } = useTranslation();
  const [confirmed, setConfirmed] = useState(false);
  const [name, setName] = useState('Studio');
  const [selected, setSelected] = useState(initiallySelected);
  const [visible, setVisible] = useState(true);
  const [lastAction, setLastAction] = useState('없음');
  const [path, setPath] = useState('/Users/MixRelay/Recordings/very-long-recording-destination');
  return (
    <div className={styles.stack}>
      <div className={styles.row}>
        <ActionButton disabled={disabled} onClick={() => setSelected(false)}>
          {t('common.cancel')}
        </ActionButton>
        <ActionButton
          emphasis="strong"
          disabled={disabled}
          loading={busy}
          onClick={() => setVisible(true)}
        >
          {t('common.create')}
        </ActionButton>
        <ActionButton
          tone="critical"
          density="compact"
          disabled={disabled}
          onClick={() => {
            setVisible(false);
            setSelected(false);
            setLastAction('삭제 샘플');
          }}
        >
          {t('studio.deleteGraph')}
        </ActionButton>
        <IconActionButton
          label={t('updates.check')}
          indicator
          disabled={disabled}
          onClick={() => setLastAction('조회 요청 샘플')}
        >
          <svg viewBox="0 0 16 16" aria-hidden="true">
            <path d="M8 2v8m-3-3 3 3 3-3M3 11v3h10v-3" />
          </svg>
        </IconActionButton>
      </div>
      <Text kind="supporting">
        샘플 명령: {lastAction}. 실제 그래프·업데이트 요청은 실행하지 않습니다.
      </Text>
      <TitleField
        disabled={disabled}
        aria-label={t('studio.graphName')}
        value={name}
        onChange={(event) => setName(event.currentTarget.value)}
      />
      {visible ? (
        <CollectionItem
          name={name}
          selected={selected}
          disabled={disabled}
          removeLabel={t('studio.deleteGraph')}
          onSelect={() => setSelected(true)}
          onRemove={() => {
            setVisible(false);
            setSelected(false);
          }}
        />
      ) : null}
      <TextField
        label={t('studio.graphName')}
        value={name}
        disabled={disabled}
        onChange={(event) => setName(event.currentTarget.value)}
      />
      <PathField
        label={t('recorder.folder')}
        value={path}
        actionLabel={t('recorder.chooseFolder')}
        disabled={disabled}
        onChoose={() => setPath('/Recordings')}
      />
      <PathField
        label={t('recorder.folder')}
        value={path}
        actionLabel={t('recorder.chooseFolder')}
        disabled
      />
      <CheckboxField
        disabled={disabled}
        label={t('driverManagement.confirmRemove')}
        checked={confirmed}
        onChange={setConfirmed}
      />
      <RetryAlert
        message={t('routing.inventory.failed')}
        retryLabel={t('routing.inventory.retry')}
        busy={disabled || busy}
        onRetry={() => setName('Recovered Studio')}
      />
    </div>
  );
}

const meta = {
  title: 'Core/Design System/System Controls',
  component: SystemControls,
} satisfies Meta<typeof SystemControls>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Interactive: Story = {};
export const Disabled: Story = { args: { disabled: true } };
export const Busy: Story = { args: { busy: true } };
export const Selected: Story = { args: { selected: true } };
export const NarrowLongTranslation: Story = {
  globals: { locale: 'pseudo' },
  decorators: [
    (Story) => (
      <div style={{ width: 320, maxWidth: '100%' }}>
        <Story />
      </div>
    ),
  ],
};
