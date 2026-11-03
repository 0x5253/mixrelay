import { useEffect, useMemo, useSyncExternalStore } from 'react';
import { useTranslation } from 'react-i18next';
import { timerScheduler } from '../../core/adapters/browser/timers';
import type {
  RecorderEncoding,
  RecorderRequest,
  RecorderStatus,
} from '../../core/domain/control/recorder';
import type { RecorderNodeInputs } from '../../core/ports/control/RecorderNodeView';
import { InlineNotice } from '../../core/ui/InlineNotice';
import { NodeOptionsSection } from '../../core/ui/NodeOptionsSection';
import { NodeTransportRow } from '../../core/ui/NodeTransportRow';
import { useNodeOptionsState } from '../../core/ui/nodeOptionsState';
import { PathField } from '../../core/ui/PathField';
import { SelectField } from '../../core/ui/SelectField';
import { TransportButton } from '../../core/ui/TransportButton';
import { RecorderController } from './application/RecorderController';
import { recordingDuration } from './domain/recording';
import styles from './RecorderControls.module.css';

const sampleRates = [44100, 48000, 88200, 96000, 176400, 192000];
const encodings: { value: RecorderEncoding; label: string }[] = [
  { value: 'pcm16', label: '16-bit PCM' },
  { value: 'pcm24', label: '24-bit PCM' },
  { value: 'float32', label: '32-bit float' },
];

export function RecorderControls(props: RecorderNodeInputs) {
  const { t } = useTranslation();
  const controller = useMemo(
    () => new RecorderController(props.controlPort, props.graphId, props.nodeId, timerScheduler),
    [props.controlPort, props.graphId, props.nodeId],
  );
  useEffect(() => {
    controller.start();
    return () => controller.dispose();
  }, [controller]);
  const snapshot = useSyncExternalStore(controller.subscribe, controller.getSnapshot);
  const settings = snapshot.settings;
  const { opened: optionsOpened, toggle: toggleOptions } = useNodeOptionsState();
  const locked = snapshot.status?.state === 'recording' || snapshot.status?.state === 'paused';
  const disabled = snapshot.busy || !snapshot.status || props.poweredOn === false;
  const fieldsLocked = disabled || locked || !props.enabled || !settings;
  return (
    <div className={styles.root}>
      <NodeTransportRow
        value={recordingDuration(snapshot.status)}
        label={t('recorder.duration')}
        state={
          snapshot.status ? t(`recorder.states.${snapshot.status.state}`) : t('recorder.loading')
        }
      >
        <RecorderButtons
          name={props.name}
          canStart={props.enabled && !!settings && !snapshot.settingsError}
          disabled={disabled}
          playbackPaused={props.playbackPaused === true}
          status={snapshot.status}
          command={(request) => controller.command(request, props.expectedRevision)}
        />
      </NodeTransportRow>
      <RecorderError error={snapshot.error ?? snapshot.settingsError} />
      <NodeOptionsSection
        disabled={props.poweredOn === false}
        label={recorderActionLabel(props.name, t('routing.nodes.options'))}
        opened={optionsOpened}
        onToggle={toggleOptions}
      >
        <ChannelsField
          channels={props.channels}
          disabled={fieldsLocked}
          onChange={props.onChannelsChange}
        />
        <PathField
          label={t('recorder.folder')}
          value={settings?.folder ?? ''}
          actionLabel={recorderActionLabel(props.name, t('recorder.chooseFolder'))}
          disabled={fieldsLocked}
          onChoose={
            props.controlPort.selectRecordingFolder
              ? () => {
                  void controller.chooseFolder(t('recorder.chooseFolder'), props.expectedRevision);
                }
              : undefined
          }
        />
        <SelectField
          label={t('recorder.sampleRate')}
          value={String((locked ? snapshot.status?.sampleRate : settings?.sampleRate) ?? 'auto')}
          disabled={fieldsLocked}
          data={[
            { value: 'auto', label: t('recorder.sourceRate') },
            ...recordingRateOptions(locked ? snapshot.status?.sampleRate : settings?.sampleRate),
          ]}
          onChange={(value) =>
            controller.saveSettings(
              { sampleRate: value === 'auto' ? null : Number(value) },
              props.expectedRevision,
            )
          }
        />
        <EncodingField
          value={(locked ? snapshot.status?.encoding : settings?.encoding) ?? 'pcm16'}
          disabled={fieldsLocked}
          onChange={(encoding) => controller.saveSettings({ encoding }, props.expectedRevision)}
        />
      </NodeOptionsSection>
    </div>
  );
}

function ChannelsField(props: {
  channels: number;
  disabled: boolean;
  onChange: (channels: number) => void;
}) {
  const { t } = useTranslation();
  return (
    <SelectField
      label={t('recorder.channels')}
      value={String(props.channels)}
      disabled={props.disabled}
      data={Array.from({ length: Math.max(8, props.channels) }, (_, index) => ({
        value: String(index + 1),
        label: String(index + 1),
      }))}
      onChange={(value) => {
        props.onChange(Number(value));
      }}
    />
  );
}

function RecorderError({ error }: { error: string | null }) {
  const { t } = useTranslation();
  return error ? (
    <InlineNotice tone="critical">
      {t('recorder.failed')}: {error}
    </InlineNotice>
  ) : null;
}

function recordingRateOptions(observed: number | null | undefined) {
  return [...new Set([...sampleRates, ...(observed ? [observed] : [])])].map((rate) => ({
    value: String(rate),
    label: `${rate / 1000} kHz`,
  }));
}

function EncodingField(props: {
  value: RecorderEncoding;
  disabled: boolean;
  onChange: (value: RecorderEncoding) => void;
}) {
  const { t } = useTranslation();
  return (
    <SelectField
      {...props}
      label={t('recorder.encoding')}
      data={encodings}
      onChange={(value) => {
        const selected = encodings.find((item) => item.value === value);
        if (selected) props.onChange(selected.value);
      }}
    />
  );
}

function RecorderButtons({
  name,
  canStart,
  disabled,
  playbackPaused,
  status,
  command,
}: {
  name?: string;
  canStart: boolean;
  disabled: boolean;
  playbackPaused: boolean;
  status: RecorderStatus | null;
  command: (request: RecorderRequest) => void;
}) {
  const { t } = useTranslation();
  const locked = status?.state === 'recording' || status?.state === 'paused';
  const label = (action: string) => recorderActionLabel(name, action);
  return (
    <>
      <TransportButton
        icon={status?.state === 'paused' ? 'resume' : 'record'}
        label={label(status?.state === 'paused' ? t('recorder.resume') : t('recorder.record'))}
        active={status?.state === 'recording'}
        disabled={
          disabled || playbackPaused || status?.state === 'recording' || (!locked && !canStart)
        }
        onClick={() => command({ action: 'record' })}
      />
      <TransportButton
        icon="pause"
        label={label(t('recorder.pause'))}
        disabled={disabled || playbackPaused || status?.state !== 'recording'}
        onClick={() => command({ action: 'pause' })}
      />
      <TransportButton
        icon="stop"
        label={label(t('recorder.stop'))}
        disabled={disabled || !locked}
        onClick={() => command({ action: 'stop' })}
      />
    </>
  );
}

function recorderActionLabel(name: string | undefined, action: string) {
  return name ? `${name}: ${action}` : action;
}
