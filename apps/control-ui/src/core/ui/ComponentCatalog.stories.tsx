import type { Meta, StoryObj } from '@storybook/react-vite';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import appIcon from '../../../../desktop/src-tauri/icons/32x32.png';
import { ActionButton } from './ActionButton';
import { AudioNodeCard } from './AudioNodeCard';
import styles from './ComponentCatalog.module.css';
import { ConfirmationDialog } from './ConfirmationDialog';
import { HierarchicalMenuButton } from './HierarchicalMenu';
import { InfoPopover } from './InfoPopover';
import { InlineSelect } from './InlineSelect';
import { LevelControl } from './LevelControl';
import { Meter } from './Meter';
import { NameDialog } from './NameDialog';
import { NodeMuteButton, NodePowerButton } from './NodeSignalControls';
import { Panel } from './Panel';
import { RecoveryActions } from './RecoveryActions';
import { RetryAlert } from './RetryAlert';
import { SelectField } from './SelectField';
import { ShortcutTooltip } from './ShortcutTooltip';
import { StateButton } from './StateButton';
import { StatusBadge } from './StatusBadge';
import { TextField } from './TextField';
import { ToggleField } from './ToggleField';
import { TransportButton } from './TransportButton';
import { UpdatesDialog } from './UpdatesDialog';
import { WorkspaceHeader } from './WorkspaceHeader';

function useCatalogModel(disabled: boolean) {
  const { t } = useTranslation();
  const [name, setName] = useState('Studio');
  const [rate, setRate] = useState('48000');
  const [gain, setGain] = useState(0);
  const [enabled, setEnabled] = useState(true);
  const [muted, setMuted] = useState(false);
  const [paused, setPaused] = useState(false);
  const [recording, setRecording] = useState(false);
  const [dialog, setDialog] = useState<string | null>(null);
  const [selection, setSelection] = useState('MixRelay');
  const close = () => setDialog(null);
  const rates = [
    { value: '44100', label: '44.1 kHz' },
    { value: '48000', label: '48 kHz' },
  ];
  const playbackStates = [
    { value: 'pause', label: t('audioSession.pause'), icon: 'Ⅱ' },
    { value: 'resume', label: t('audioSession.resume'), icon: '▷' },
    { value: 'pending', label: t('audioSession.changing'), icon: 'Ⅱ' },
  ];
  return {
    t,
    name,
    setName,
    rate,
    setRate,
    gain,
    setGain,
    enabled,
    setEnabled,
    muted,
    setMuted,
    paused,
    setPaused,
    recording,
    setRecording,
    dialog,
    setDialog,
    selection,
    setSelection,
    close,
    rates,
    playbackStates,
    disabled,
  };
}

type CatalogModel = ReturnType<typeof useCatalogModel>;

function CatalogCommands({ model }: { model: CatalogModel }) {
  const {
    t,
    name,
    enabled,
    setEnabled,
    muted,
    setMuted,
    paused,
    setPaused,
    recording,
    setRecording,
    setDialog,
    setSelection,
    playbackStates,
    disabled,
  } = model;
  return (
    <Panel title="Commands">
      <div className={styles.stack}>
        <WorkspaceHeader>
          <span>{name}</span>
          <StateButton
            states={playbackStates}
            value={paused ? 'resume' : 'pause'}
            tone={paused ? 'neutral' : 'positive'}
            aria-pressed={paused}
            disabled={disabled}
            onClick={() => setPaused(!paused)}
          />
        </WorkspaceHeader>
        <div className={styles.row}>
          <ActionButton disabled={disabled} onClick={() => setDialog('name')}>
            + {t('studio.newGraph')}
          </ActionButton>
          <ActionButton disabled={disabled} emphasis="strong" onClick={() => setSelection(name)}>
            {t('driverManagement.save')}
          </ActionButton>
          <ActionButton disabled={disabled} tone="critical" onClick={() => setDialog('confirm')}>
            {t('studio.deleteGraph')}
          </ActionButton>
        </div>
        <div className={styles.row}>
          <HierarchicalMenuButton
            label={t('workspaceFile.file')}
            buttonContent={t('workspaceFile.file')}
            items={[
              { id: 'save', label: t('workspaceFile.save'), disabled },
              {
                id: 'outputs',
                label: t('studio.stream'),
                children: [{ id: 'io', label: 'MixRelay IO', description: '2 CH · 48 kHz' }],
              },
            ]}
            onSelect={setSelection}
          />
          <ShortcutTooltip shortcut="Mod+S">
            <ActionButton density="compact" disabled={disabled} onClick={() => setSelection(name)}>
              {t('workspaceFile.save')}
            </ActionButton>
          </ShortcutTooltip>
          <InfoPopover label={t('audioSession.diagnostics')}>
            <p className={styles.note}>48 kHz · 128 frames · 2.67 ms</p>
          </InfoPopover>
        </div>
        <div className={styles.row}>
          <TransportButton
            icon="record"
            label={t('recorder.record')}
            disabled={disabled}
            active={recording}
            onClick={() => setRecording(!recording)}
          />
          <TransportButton
            icon="pause"
            label={t('recorder.pause')}
            disabled={disabled}
            active={recording && paused}
            onClick={() => setPaused(!paused)}
          />
          <TransportButton
            icon="stop"
            label={t('recorder.stop')}
            disabled={disabled}
            onClick={() => setRecording(false)}
          />
          <NodePowerButton
            enabled={enabled}
            label={t(`routing.nodeControl.${enabled ? 'powerOff' : 'powerOn'}`, {
              name: 'MixRelay',
            })}
            disabled={disabled}
            onChange={() => setEnabled(!enabled)}
          />
          <NodeMuteButton
            muted={muted}
            label={t(`routing.nodeControl.${muted ? 'unmute' : 'mute'}`, { name: 'MixRelay' })}
            disabled={disabled}
            onChange={() => setMuted(!muted)}
          />
        </div>
        <StateButton states={playbackStates} value="pending" tone="warning" disabled aria-busy />
      </div>
    </Panel>
  );
}

function CatalogInputs({ model }: { model: CatalogModel }) {
  const { t, name, setName, rate, setRate, gain, setGain, enabled, setEnabled, rates, disabled } =
    model;
  return (
    <Panel title="Inputs">
      <div className={styles.stack}>
        <TextField
          label={t('studio.graphName')}
          value={name}
          disabled={disabled}
          onChange={(event) => setName(event.currentTarget.value)}
        />
        <SelectField
          label={t('audioSession.sampleRate')}
          data={rates}
          value={rate}
          disabled={disabled}
          onChange={setRate}
        />
        <InlineSelect
          label={t('recorder.sourceRate')}
          data={rates}
          value={rate}
          disabled={disabled}
          onChange={setRate}
        />
        <ToggleField
          label={t('studio.capture')}
          checked={enabled}
          disabled={disabled}
          onChange={setEnabled}
        />
        <LevelControl
          label={t('routing.nodes.gain')}
          value={gain}
          suffix="dB"
          min={-60}
          max={12}
          disabled={disabled}
          onChange={setGain}
        />
      </div>
    </Panel>
  );
}

function CatalogFeedback({ model }: { model: CatalogModel }) {
  const { t, setSelection, disabled } = model;
  return (
    <Panel title="Feedback">
      <div className={styles.stack}>
        <div className={styles.row}>
          <StatusBadge>{t('recorder.states.stopped')}</StatusBadge>
          <StatusBadge tone="positive">{t('common.ready')}</StatusBadge>
          <StatusBadge tone="warning">{t('recorder.states.paused')}</StatusBadge>
          <StatusBadge tone="critical">{t('recorder.failed')}</StatusBadge>
        </div>
        <div className={styles.meter}>
          <Meter label="MixRelay / CH 1" level={72} orientation="horizontal" />
        </div>
        <TextField
          label={t('studio.graphName')}
          value=""
          onChange={() => {}}
          error={t('driverManagement.changeFailed')}
        />
        <TextField label={t('driverManagement.name')} value="MixRelay IO" readOnly />
        <RetryAlert
          message={t('studio.catalogUnavailable')}
          retryLabel={t('runtimeConnection.retry')}
          busy={disabled}
          onRetry={() => setSelection('Retry')}
        />
        <RecoveryActions
          retryLabel={t('runtimeConnection.retry')}
          reloadLabel={t('studio.reloadGraphs')}
          onRetry={() => setSelection('Retry')}
          onReload={() => setSelection('Reload')}
        />
      </div>
    </Panel>
  );
}

function CatalogNodes({ model }: { model: CatalogModel }) {
  const {
    t,
    gain,
    setGain,
    enabled,
    setEnabled,
    muted,
    setMuted,
    setDialog,
    selection,
    setSelection,
    disabled,
  } = model;
  return (
    <Panel title="Nodes & dialogs">
      <div className={styles.stack}>
        <div className={styles.node}>
          <AudioNodeCard
            title="MixRelay"
            category="Application"
            icon="APP"
            iconUrl={appIcon}
            inputCount={0}
            outputCount={2}
            showPorts={false}
            selected={selection === 'MixRelay'}
            onSelect={() => setSelection('MixRelay')}
            poweredOff={!enabled}
            headerActions={
              <NodePowerButton
                enabled={enabled}
                disabled={disabled}
                label={t(enabled ? 'routing.nodeControl.powerOff' : 'routing.nodeControl.powerOn', {
                  name: 'MixRelay',
                })}
                onChange={() => setEnabled(!enabled)}
              />
            }
            muteControl={
              <NodeMuteButton
                muted={muted}
                disabled={disabled}
                label={t(muted ? 'routing.nodeControl.unmute' : 'routing.nodeControl.mute', {
                  name: 'MixRelay',
                })}
                onChange={() => setMuted(!muted)}
              />
            }
            gainControl={{ label: t('routing.nodes.gain'), valueDb: gain }}
            onGainChange={setGain}
            optionsLabel={t('routing.nodes.options')}
            status={t('common.ready')}
            statusTone="positive"
          />
        </div>
        <div className={styles.row}>
          <ActionButton onClick={() => setDialog('updates')}>{t('updates.title')}</ActionButton>
        </div>
      </div>
    </Panel>
  );
}

function CatalogDialogs({ model }: { model: CatalogModel }) {
  const { t, name, setName, dialog, setSelection, close } = model;
  return (
    <>
      {dialog === 'name' ? (
        <NameDialog
          title={t('studio.newGraph')}
          initialName={name}
          nameLabel={t('studio.graphName')}
          createLabel={t('common.create')}
          enabled
          onCancel={close}
          onCreate={(value) => {
            setName(value);
            close();
          }}
        />
      ) : null}
      <ConfirmationDialog
        opened={dialog === 'confirm'}
        title={t('studio.deleteGraph')}
        busy={false}
        disabled={false}
        cancelLabel={t('common.cancel')}
        onCancel={close}
        onConfirm={() => {
          setSelection('');
          close();
        }}
      >
        <p className={styles.note}>{t('studio.deleteGraphNotice', { name })}</p>
      </ConfirmationDialog>
      <UpdatesDialog
        opened={dialog === 'updates'}
        title={t('updates.title')}
        closeLabel={t('updates.close')}
        onClose={close}
      >
        <StatusBadge tone="positive">{t('updates.states.current')}</StatusBadge>
      </UpdatesDialog>
    </>
  );
}

function ComponentCatalog({ disabled = false }: { disabled?: boolean }) {
  const model = useCatalogModel(disabled);
  return (
    <main className={styles.root}>
      <h1 className={styles.heading}>MixRelay / Components</h1>
      <div className={styles.grid}>
        <CatalogCommands model={model} />
        <CatalogInputs model={model} />
        <CatalogFeedback model={model} />
        <CatalogNodes model={model} />
      </div>
      <CatalogDialogs model={model} />
    </main>
  );
}

const meta = {
  title: 'Core/Design System/Component Catalog',
  component: ComponentCatalog,
} satisfies Meta<typeof ComponentCatalog>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Overview: Story = {};
export const Disabled: Story = { args: { disabled: true } };
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
