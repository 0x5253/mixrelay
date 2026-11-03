import type { Meta, StoryObj } from '@storybook/react-vite';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ActionButton } from './ActionButton';
import { ContentSection } from './ContentSection';
import { FormActions } from './FormActions';
import { Inline } from './Inline';
import { InlineNotice } from './InlineNotice';
import { SelectField } from './SelectField';
import { Stack } from './Stack';
import { Text } from './Text';
import { TextField } from './TextField';

function FormFlow({ failure = false, busy = false }: { failure?: boolean; busy?: boolean }) {
  const { t } = useTranslation();
  const [name, setName] = useState('Studio');
  const [rate, setRate] = useState('48000');
  const [saved, setSaved] = useState(false);
  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        setSaved(true);
      }}
    >
      <Stack>
        <TextField
          label={t('driverManagement.name')}
          value={name}
          disabled={busy}
          onChange={(event) => {
            setName(event.currentTarget.value);
            setSaved(false);
          }}
        />
        <SelectField
          label={t('audioSession.sampleRate')}
          value={rate}
          disabled={busy}
          data={[
            { value: '44100', label: '44.1 kHz' },
            { value: '48000', label: '48 kHz' },
          ]}
          onChange={(value) => {
            setRate(value);
            setSaved(false);
          }}
        />
        {failure ? (
          <InlineNotice
            tone="critical"
            title={t('updates.openFailed')}
            actions={
              <ActionButton disabled={busy}>{t('routing.processorControl.retry')}</ActionButton>
            }
          >
            {t('updates.installNotice')}
          </InlineNotice>
        ) : null}
        {saved ? (
          <InlineNotice>
            {name} · {Number(rate) / 1000} kHz
          </InlineNotice>
        ) : null}
      </Stack>
      <FormActions>
        <ActionButton disabled={busy} onClick={() => setSaved(false)}>
          {t('common.cancel')}
        </ActionButton>
        <ActionButton
          type="submit"
          loading={busy}
          disabled={busy || !name.trim()}
          emphasis="strong"
        >
          {t('driverManagement.save')}
        </ActionButton>
      </FormActions>
    </form>
  );
}

const meta = { title: 'Core/Composition/Form Flow', component: FormFlow } satisfies Meta<
  typeof FormFlow
>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Interactive: Story = {};
export const Failed: Story = { args: { failure: true } };
export const Pending: Story = { args: { busy: true } };
export const NarrowLongTranslation: Story = {
  args: { failure: true },
  globals: { locale: 'pseudo' },
  decorators: [
    (Story) => (
      <div style={{ maxWidth: 280 }}>
        <Story />
      </div>
    ),
  ],
};
export const NoticeStates: Story = {
  render: () => (
    <Stack gap="section">
      <InlineNotice title="Status">MixRelay · 48 kHz</InlineNotice>
      <InlineNotice tone="warning" title="Paused">
        Recording excludes the paused interval.
      </InlineNotice>
      <InlineNotice
        tone="critical"
        title="Connection failed"
        actions={<ActionButton>Retry</ActionButton>}
      >
        The last confirmed device settings are retained.
      </InlineNotice>
      <Inline justify="between">
        <span>Device</span>
        <span>2 CH · 48 kHz</span>
      </Inline>
    </Stack>
  ),
};

export const ContentGroups: Story = {
  render: () => (
    <Stack gap="section">
      <ContentSection title="MixRelay Driver" divider="after">
        <Text kind="supporting" tone="muted">
          Installed version · 0.1.3
        </Text>
        <Inline justify="between">
          <Text as="span">MixRelay IO</Text>
          <Text as="span" kind="supporting">
            2 CH · 48 kHz
          </Text>
        </Inline>
      </ContentSection>
      <ContentSection title="Device" level={3} divider="before">
        <Text>Device settings are confirmed by MixRelay Driver.</Text>
        <Text kind="caption" tone="muted">
          Current values remain visible during a pending request.
        </Text>
      </ContentSection>
    </Stack>
  ),
};
