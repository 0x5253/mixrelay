import type { Meta, StoryObj } from '@storybook/react-vite';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import appIcon from '../../../../desktop/src-tauri/icons/32x32.png';
import { AudioNodeCard } from './AudioNodeCard';
import { InlineNotice } from './InlineNotice';
import { NodeOptionsSection } from './NodeOptionsSection';
import { NodeMuteButton, NodePowerButton } from './NodeSignalControls';
import { NodeTransportRow } from './NodeTransportRow';
import { TextField } from './TextField';
import { TransportButton } from './TransportButton';

function FrameExample({
  channels,
  recorder,
  off,
  longName,
  recording,
  failed,
}: {
  channels: number;
  recorder?: boolean;
  off?: boolean;
  longName?: boolean;
  recording?: boolean;
  failed?: boolean;
}) {
  const { t } = useTranslation();
  const [enabled, setEnabled] = useState(!off);
  const [muted, setMuted] = useState(false);
  const [gain, setGain] = useState(0);
  const title = longName
    ? 'Multichannel audiovisual performance — Main stage monitor'
    : recorder
      ? 'Recorder'
      : `${channels} CH source`;
  return (
    <div style={{ width: 'var(--node-default-width)', maxWidth: '100%' }}>
      <AudioNodeCard
        title={title}
        category={recorder ? 'Recorder' : 'Application'}
        icon={recorder ? 'REC' : 'APP'}
        iconUrl={recorder ? undefined : appIcon}
        inputCount={recorder ? channels : 0}
        outputCount={recorder ? 0 : channels}
        channelCount={channels}
        detail={recorder ? undefined : `${gain.toFixed(1)} dB`}
        poweredOff={!enabled}
        faceMode={recorder ? 'channel-rows' : 'standard'}
        meterBanks={[{ nodeId: title, portId: 'audio', channels }]}
        showPorts={false}
        headerActions={
          <NodePowerButton
            enabled={enabled}
            disabled={false}
            label={t(enabled ? 'routing.nodeControl.powerOff' : 'routing.nodeControl.powerOn', {
              name: title,
            })}
            onChange={() => setEnabled(!enabled)}
          />
        }
        muteControl={
          <NodeMuteButton
            muted={muted}
            disabled={false}
            label={t(muted ? 'routing.nodeControl.unmute' : 'routing.nodeControl.mute', {
              name: title,
            })}
            onChange={() => setMuted(!muted)}
          />
        }
        optionsLabel={t('routing.nodes.options')}
        gainControl={recorder ? undefined : { label: 'Level', valueDb: gain }}
        onGainChange={setGain}
        content={
          recorder ? (
            <RecorderPreview
              enabled={enabled}
              recording={recording === true}
              failed={failed === true}
            />
          ) : undefined
        }
      />
    </div>
  );
}

function RecorderPreview({
  enabled,
  recording,
  failed,
}: {
  enabled: boolean;
  recording: boolean;
  failed: boolean;
}) {
  const { t } = useTranslation();
  const [opened, setOpened] = useState(false);
  return (
    <>
      <NodeTransportRow value="00:00:00.000" label={t('recorder.duration')}>
        <TransportButton
          icon="record"
          label={t('recorder.record')}
          active={recording}
          disabled={!enabled || recording}
          onClick={() => {}}
        />
        <TransportButton
          icon="pause"
          label={t('recorder.pause')}
          disabled={!enabled || !recording}
          onClick={() => {}}
        />
        <TransportButton
          icon="stop"
          label={t('recorder.stop')}
          disabled={!enabled || !recording}
          onClick={() => {}}
        />
      </NodeTransportRow>
      {failed ? (
        <InlineNotice tone="critical">
          {t('recorder.failed')}: Output folder unavailable.
        </InlineNotice>
      ) : null}
      <NodeOptionsSection
        label={t('routing.nodes.options')}
        opened={opened}
        onToggle={() => setOpened(!opened)}
        disabled={!enabled}
      >
        <TextField label={t('recorder.folder')} value="/Music/Recordings" readOnly />
      </NodeOptionsSection>
    </>
  );
}

function NodeFrames({
  channels = [1, 2, 4],
  off = false,
  longName = false,
  recording = false,
  failed = false,
}: {
  channels?: number[];
  off?: boolean;
  longName?: boolean;
  recording?: boolean;
  failed?: boolean;
}) {
  return (
    <div style={{ display: 'flex', flexWrap: 'wrap', alignItems: 'flex-start', gap: 16 }}>
      {channels.map((count) => (
        <FrameExample key={count} channels={count} off={off} longName={longName} />
      ))}
      <FrameExample
        channels={2}
        recorder
        off={off}
        longName={longName}
        recording={recording}
        failed={failed}
      />
    </div>
  );
}

const meta = { title: 'Core/Audio Nodes/Frame', component: NodeFrames } satisfies Meta<
  typeof NodeFrames
>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Default: Story = {};
export const Multichannel: Story = { args: { channels: [8, 16, 32, 64] } };
export const Off: Story = { args: { off: true } };
export const Recording: Story = { args: { recording: true } };
export const Failure: Story = { args: { failed: true } };
export const NarrowLongTranslation: Story = {
  args: { longName: true },
  globals: { locale: 'pseudo' },
  parameters: { viewport: { value: 'mobile' } },
};
