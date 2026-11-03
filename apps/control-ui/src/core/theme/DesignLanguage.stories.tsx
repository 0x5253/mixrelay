import type { Meta, StoryObj } from '@storybook/react-vite';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ActionButton } from '../ui/ActionButton';
import { ContentSection } from '../ui/ContentSection';
import { FormActions } from '../ui/FormActions';
import { Inline } from '../ui/Inline';
import { InlineNotice } from '../ui/InlineNotice';
import { NodeMuteButton, NodePowerButton } from '../ui/NodeSignalControls';
import { Stack } from '../ui/Stack';
import { StatusBadge } from '../ui/StatusBadge';
import { Text } from '../ui/Text';
import { TextField } from '../ui/TextField';
import { ThemePicker } from '../ui/ThemePicker';
import { TransportButton } from '../ui/TransportButton';

function ProductSymbols() {
  const { t } = useTranslation();
  const [enabled, setEnabled] = useState(true);
  const [muted, setMuted] = useState(false);
  const [recording, setRecording] = useState(false);
  const [paused, setPaused] = useState(false);
  return (
    <ContentSection title="아이콘 · 상태를 구별하는 실제 제품 조작">
      <Text kind="supporting" tone="muted">
        아이콘을 다시 그린 견본이 아니라 Core의 전원·음소거·녹음·일시정지·정지 버튼입니다. 상태와
        키보드 포커스는 별개입니다.
      </Text>
      <Inline>
        <NodePowerButton
          enabled={enabled}
          disabled={false}
          label={t(`routing.nodeControl.${enabled ? 'powerOff' : 'powerOn'}`, { name: 'MixRelay' })}
          onChange={() => setEnabled((value) => !value)}
        />
        <NodeMuteButton
          muted={muted}
          disabled={false}
          label={t(`routing.nodeControl.${muted ? 'unmute' : 'mute'}`, { name: 'MixRelay' })}
          onChange={() => setMuted((value) => !value)}
        />
        <Text kind="caption">
          {enabled ? 'On' : 'Off'} · {muted ? 'Mute' : 'Unmuted'}
        </Text>
      </Inline>
      <Inline>
        <TransportButton
          icon="record"
          label={t('recorder.record')}
          active={recording}
          disabled={recording}
          onClick={() => {
            setRecording(true);
            setPaused(false);
          }}
        />
        <TransportButton
          icon={paused ? 'resume' : 'pause'}
          label={t(`recorder.${paused ? 'resume' : 'pause'}`)}
          active={paused}
          disabled={!recording}
          onClick={() => setPaused((value) => !value)}
        />
        <TransportButton
          icon="stop"
          label={t('recorder.stop')}
          disabled={!recording}
          onClick={() => {
            setRecording(false);
            setPaused(false);
          }}
        />
        <StatusBadge tone={recording ? (paused ? 'warning' : 'critical') : 'neutral'}>
          {t(`recorder.states.${recording ? (paused ? 'paused' : 'recording') : 'stopped'}`)}
        </StatusBadge>
      </Inline>
      <Text kind="caption">
        Core 상태 견본이며 오디오나 파일은 생성하지 않습니다. 실제 노드 바이패스·옵션은
        Features/Builtin Effects에서 동일 제품 노드로 검토합니다.
      </Text>
    </ContentSection>
  );
}

function DesignLanguage() {
  const { t } = useTranslation();
  const [name, setName] = useState('MixRelay IO');
  const [graphName, setGraphName] = useState('');
  return (
    <Stack gap="section">
      <Inline>
        <ThemePicker />
        <StatusBadge>{t('theme.label')}</StatusBadge>
      </Inline>
      <ContentSection title="상태 · 색과 문구의 대응">
        <Inline>
          <StatusBadge>{t('updates.states.not_published')}</StatusBadge>
          <StatusBadge tone="accent">{t('updates.states.available')}</StatusBadge>
          <StatusBadge tone="positive">{t('updates.states.current')}</StatusBadge>
          <StatusBadge tone="warning">{t('driverManagement.saveFailed')}</StatusBadge>
          <StatusBadge tone="critical">{t('driverManagement.changeFailed')}</StatusBadge>
        </Inline>
        <InlineNotice tone="neutral">{t('updates.states.not_published')}</InlineNotice>
        <InlineNotice tone="accent">{t('updates.states.available')}</InlineNotice>
        <InlineNotice tone="positive">{t('updates.states.current')}</InlineNotice>
        <InlineNotice tone="warning">{t('driverManagement.saveFailed')}</InlineNotice>
        <InlineNotice tone="critical">{t('driverManagement.changeFailed')}</InlineNotice>
      </ContentSection>
      <ContentSection title="입력 · 명령 · 포커스">
        <Text kind="supporting" tone="muted">
          Tab으로 실제 포커스, 포인터로 hover/pressed를 확인합니다. 입력은 경계 하나, 버튼은 중립색
          윤곽 하나를 사용하는 제품 규격입니다.
        </Text>
        <TextField
          label={t('driverManagement.name')}
          value={name}
          onChange={(event) => setName(event.currentTarget.value)}
        />
        <TextField
          label={t('studio.graphName')}
          value={graphName}
          onChange={(event) => setGraphName(event.currentTarget.value)}
          error={t('studio.graphSaveFailed')}
        />
        <FormActions>
          <ActionButton
            onClick={() => {
              setName('MixRelay IO');
              setGraphName('');
            }}
          >
            {t('common.cancel')}
          </ActionButton>
          <ActionButton tone="critical" onClick={() => setName('')}>
            {t('driverManagement.remove')}
          </ActionButton>
          <ActionButton emphasis="strong" onClick={() => setGraphName(name)}>
            {t('driverManagement.save')}
          </ActionButton>
          <ActionButton emphasis="strong" disabled>
            {t('driverManagement.save')}
          </ActionButton>
        </FormActions>
      </ContentSection>
      <ProductSymbols />
    </Stack>
  );
}

const meta = {
  title: 'Core/Design System/Design Language',
  component: DesignLanguage,
} satisfies Meta<typeof DesignLanguage>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Interactive: Story = {};
export const Light: Story = { globals: { theme: 'light' } };
export const Dark: Story = { globals: { theme: 'dark' } };
export const NarrowLongTranslation: Story = {
  globals: { locale: 'pseudo', theme: 'light', viewport: { value: 'mobile' } },
};
export const SymbolsAndStates: Story = { render: () => <ProductSymbols /> };
