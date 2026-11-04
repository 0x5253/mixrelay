import { useEffect, useMemo, useSyncExternalStore } from 'react';
import { useTranslation } from 'react-i18next';
import { timerScheduler } from '../../core/adapters/browser/timers';
import type { RecorderEncoding, RecorderRequest } from '../../core/domain/control/recorder';
import type { RecorderNodeInputs } from '../../core/ports/control/RecorderNodeView';
import { InlineNotice } from '../../core/ui/InlineNotice';
import { NodeOptionsSection } from '../../core/ui/NodeOptionsSection';
import { NodeTransportRow } from '../../core/ui/NodeTransportRow';
import { useNodeOptionsState } from '../../core/ui/nodeOptionsState';
import { PathField } from '../../core/ui/PathField';
import { SelectField } from '../../core/ui/SelectField';
import { TransportButton } from '../../core/ui/TransportButton';
import { RecorderController } from './application/RecorderController';
import {
  type RecorderControlState,
  recorderControlState,
  recordingRates,
} from './domain/recorderControlState';
import { recordingDuration } from './domain/recording';
import styles from './RecorderControls.module.css';

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
  const view = recorderControlState({
    ...snapshot,
    enabled: props.enabled,
    poweredOn: props.poweredOn !== false,
    playbackPaused: props.playbackPaused === true,
  });
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
          view={view}
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
          disabled={view.fieldsLocked}
          onChange={props.onChannelsChange}
        />
        <PathField
          label={t('recorder.folder')}
          value={settings?.folder ?? ''}
          actionLabel={recorderActionLabel(props.name, t('recorder.chooseFolder'))}
          disabled={view.fieldsLocked}
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
          value={String(view.sampleRate ?? 'auto')}
          disabled={view.fieldsLocked}
          data={[
            { value: 'auto', label: t('recorder.sourceRate') },
            ...recordingRates(view.sampleRate).map((rate) => ({
              value: String(rate),
              label: `${rate / 1000} kHz`,
            })),
          ]}
          onChange={(value) =>
            controller.saveSettings(
              { sampleRate: value === 'auto' ? null : Number(value) },
              props.expectedRevision,
            )
          }
        />
        <EncodingField
          value={view.encoding}
          disabled={view.fieldsLocked}
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
  view,
  command,
}: {
  name?: string;
  view: RecorderControlState;
  command: (request: RecorderRequest) => void;
}) {
  const { t } = useTranslation();
  const label = (action: string) => recorderActionLabel(name, action);
  return (
    <>
      <TransportButton
        icon={view.record.resumes ? 'resume' : 'record'}
        label={label(view.record.resumes ? t('recorder.resume') : t('recorder.record'))}
        active={view.record.active}
        disabled={!view.record.enabled}
        onClick={() => command({ action: 'record' })}
      />
      <TransportButton
        icon="pause"
        label={label(t('recorder.pause'))}
        disabled={!view.pause.enabled}
        onClick={() => command({ action: 'pause' })}
      />
      <TransportButton
        icon="stop"
        label={label(t('recorder.stop'))}
        disabled={!view.stop.enabled}
        onClick={() => command({ action: 'stop' })}
      />
    </>
  );
}

function recorderActionLabel(name: string | undefined, action: string) {
  return name ? `${name}: ${action}` : action;
}
