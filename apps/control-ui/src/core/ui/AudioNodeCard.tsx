import {
  type CSSProperties,
  type ReactNode,
  type Ref,
  useContext,
  useEffect,
  useState,
} from 'react';
import { Button } from 'react-aria-components';
import { useTranslation } from 'react-i18next';
import { MidiContext } from '../midi/MidiContext';
import { useMidiControl } from '../midi/useMidiControl';
import { spacing } from '../theme/tokens';
import { ActionButton } from './ActionButton';
import styles from './AudioNodeCard.module.css';
import {
  AudioNodeEndpointOptions,
  type EndpointControl,
  type EndpointUpdate,
} from './AudioNodeEndpointOptions';
import { AudioNodeIcon } from './AudioNodeIcon';
import { type AudioNodeMeterBank, AudioNodeMeterBanks } from './AudioNodeMeterBanks';
import { type AudioNodeParameterControl, AudioNodeParameterGrid } from './AudioNodeParameterGrid';
import {
  type AudioNodeFaceMode,
  audioNodeFaceGeometry,
  audioNodeFrame,
  channelRowMetrics,
} from './audioNodeLayout';
import { DeviceAppActivity, type DeviceAppActivityProps } from './DeviceAppActivity';
import { DynamicsMeter, type DynamicsMeterContract } from './DynamicsMeter';
import { GraphicEqualizer, type GraphicEqualizerContract } from './GraphicEqualizer';
import { IconActionButton } from './IconActionButton';
import {
  NodeDisclosureMeasureContext,
  NodeHeaderOptionsContext,
  NodeOptionsSection,
} from './NodeOptionsSection';
import { useNodeOptionsState } from './nodeOptionsState';
import { ProcessorTimingLabel } from './ProcessorTimingLabel';
import { SelectField } from './SelectField';
import type { StatusTone } from './StatusBadge';
import { StatusIndicator } from './StatusIndicator';
import { Text } from './Text';
import { Tooltip } from './Tooltip';
import { useCardMeasurements } from './useAudioNodeCardMeasurements';
import { useElementBounds } from './useElementBounds';
import { useNodeHeaderFocus } from './useNodeHeaderFocus';
import { ValueSlider } from './ValueSlider';

const portIds = ['one', 'two', 'three', 'four', 'five', 'six', 'seven', 'eight'];
/** A node setting chosen from a fixed list, such as a plug-in's channel layout. */
export interface NodeChoiceControl {
  label: string;
  value: string;
  options: readonly { value: string; label: string; description?: string }[];
}

export interface AudioNodeCardProps {
  deviceAppActivity?: DeviceAppActivityProps;
  equalizer?: GraphicEqualizerContract;
  onParametersChange?: (updates: readonly { id: string; value: number }[]) => void;
  headerActions?: ReactNode;
  muteControl?: ReactNode;
  poweredOff?: boolean;
  processingTimeNs?: number;
  content?: ReactNode;
  bypassed?: boolean;
  /** Device presence only; selection and configuration remain available. */
  offline?: boolean;
  category: string;
  channelCount?: number;
  detail?: string;
  endpointControl?: EndpointControl;
  /** Offered in Options when there is more than one choice. */
  channelLayoutControl?: NodeChoiceControl;
  faceHeight?: number;
  faceMode?: AudioNodeFaceMode;
  onFaceHeightChange?: (height: number) => void;
  onContentHeightChange?: (height: number, disclosureHeight: number) => void;
  gainControl?: { label: string; valueDb: number };
  icon: string;
  iconUrl?: string;
  inputCount: number;
  meterBanks?: readonly AudioNodeMeterBank[];
  outputCount: number;
  optionsLabel?: string;
  parameterControls?: readonly AudioNodeParameterControl[];
  dynamics?: DynamicsMeterContract;
  processorEditor?: {
    actionLabel: string;
    failureLabel?: string;
    state: 'unavailable' | 'closed' | 'opening' | 'open' | 'failed';
  };
  showPorts?: boolean;
  selected?: boolean;
  status?: string;
  statusTone?: StatusTone;
  title: string;
  /** Omit when a parent graph owns node selection and keyboard focus. */
  onSelect?: () => void;
  onGainChange?: (gainDb: number) => void;
  onEndpointChange?: (value: EndpointUpdate) => void;
  onChannelLayoutChange?: (value: string) => void;
  onLayoutChange?: () => void;
  onParameterChange?: (parameterId: string, value: number) => void;
  onProcessorEditorRetry?: () => void;
}

function meterBanksForCompactFace(
  faceMode: AudioNodeFaceMode,
  meterBanks: readonly AudioNodeMeterBank[],
) {
  return faceMode === 'compact' ? meterBanks : [];
}

function toggleNodeOptions(toggle: () => void, onLayoutChange?: () => void) {
  toggle();
  requestAnimationFrame(() => onLayoutChange?.());
}

export function AudioNodeCard({
  deviceAppActivity,
  headerActions,
  muteControl,
  poweredOff = false,
  processingTimeNs,
  bypassed = false,
  offline = false,
  category,
  content,
  channelCount = 0,
  detail,
  faceHeight,
  faceMode = 'standard',
  onFaceHeightChange,
  onContentHeightChange,
  icon,
  iconUrl,
  inputCount,
  meterBanks = [],
  outputCount,
  optionsLabel,
  onSelect,
  onLayoutChange,
  onParameterChange,
  onProcessorEditorRetry,
  parameterControls = [],
  equalizer,
  dynamics,
  onParametersChange,
  processorEditor,
  selected = false,
  showPorts = true,
  status,
  statusTone = 'neutral',
  title,
  // Endpoint, channel layout and gain settings, which only the Options panel reads.
  ...settings
}: AudioNodeCardProps) {
  const mapping = useContext(MidiContext)?.active ?? false;
  const settingsDisabled = poweredOff || mapping;
  const { t } = useTranslation();
  const resolvedOptionsLabel = optionsLabel ?? t('routing.nodes.options');
  const { faceRef, faceBounds, contentRef, measureControls } = useCardMeasurements({
    onFaceHeightChange,
    onContentHeightChange,
  });
  const { opened: optionsOpened, toggle } = useNodeOptionsState();
  const [headerOptionsTarget, setHeaderOptionsTarget] = useState<HTMLDivElement | null>(null);
  const [headerRef, headerBounds] = useElementBounds<HTMLDivElement>();
  const { hasOptions, hasEndpointControl, hasParameterControls, channelLayout } =
    nodeOptionsAvailability({ ...settings, parameterControls, onParameterChange, processorEditor });
  const editor = { title, control: processorEditor, onRetry: onProcessorEditorRetry };
  return (
    <div
      className={`${styles.root} nokey`}
      data-equalizer={Boolean(equalizer) || undefined}
      {...cardStateAttributes({ bypassed, offline, poweredOff, selected, dynamics })}
      data-endpoint-options={hasEndpointControl || undefined}
      data-has-options={hasOptions || undefined}
      data-header-actions={Boolean(headerActions)}
      data-metered={meterBanks.length > 0 || undefined}
      data-inline-controls={Boolean(content) || faceMode === 'channel-rows' || undefined}
      data-face-mode={faceMode}
      data-options-opened={optionsOpened || undefined}
      style={cardRootStyle({ faceHeight, faceMode, meterBanks, faceBounds, headerBounds })}
    >
      <NodeDisclosureMeasureContext.Provider value={measureControls}>
        <NodeHeaderOptionsContext.Provider value={headerOptionsTarget}>
          <AudioNodeFace
            processingTimeNs={processingTimeNs}
            faceRef={faceRef}
            category={category}
            channelCount={channelCount}
            hideSummary={Boolean(content) || faceMode !== 'standard'}
            deviceAppActivity={deviceAppActivity}
            detail={detail}
            icon={icon}
            iconUrl={iconUrl}
            inputCount={inputCount}
            meterBanks={dynamics ? [] : meterBanks}
            faceMode={faceMode}
            offline={offline}
            onSelect={onSelect}
            outputCount={outputCount}
            selected={selected}
            status={status}
            statusTone={statusTone}
            title={title}
          />
          <NodeHeaderActions
            {...{ title, actions: headerActions, mute: muteControl }}
            bypassControls={parameterControls}
            bypassDisabled={settingsDisabled}
            onBypassChange={onParameterChange}
            headerRef={headerRef}
            targetRef={setHeaderOptionsTarget}
          />
          <AudioNodeControls
            content={content}
            compactMeterBanks={dynamics ? [] : meterBanksForCompactFace(faceMode, meterBanks)}
            dynamics={dynamics}
            parameterControls={parameterControls}
            bypassed={bypassed}
            faceMode={faceMode}
            measureRef={contentRef}
            offline={offline || poweredOff}
            title={title}
          >
            <CompactNodeFeedback {...{ faceMode, detail, status, statusTone }} />
            <div inert={settingsDisabled}>
              <ProcessorEditorOption {...editor} />
            </div>
            <NodeOptions
              disabled={settingsDisabled}
              {...settings}
              channelLayout={channelLayout}
              hasOptions={hasOptions}
              label={`${title}: ${resolvedOptionsLabel}`}
              onParameterChange={onParameterChange}
              onToggle={() => toggleNodeOptions(toggle, onLayoutChange)}
              opened={optionsOpened}
              parameterControls={hasParameterControls ? parameterControls : []}
              equalizer={equalizer}
              onParametersChange={onParametersChange}
            />
          </AudioNodeControls>
          <NodePortOverlays inputCount={inputCount} outputCount={outputCount} visible={showPorts} />
        </NodeHeaderOptionsContext.Provider>
      </NodeDisclosureMeasureContext.Provider>
    </div>
  );
}

function cardStateAttributes({
  bypassed,
  offline,
  poweredOff,
  selected,
  dynamics,
}: Pick<AudioNodeCardProps, 'bypassed' | 'offline' | 'poweredOff' | 'selected' | 'dynamics'>) {
  return {
    'data-bypassed': bypassed || undefined,
    'data-offline': offline || undefined,
    'data-powered-off': poweredOff,
    'data-selected': selected || undefined,
    'data-dynamics': Boolean(dynamics) || undefined,
  };
}

function cardRootStyle(
  props: Parameters<typeof audioNodeCardStyle>[0] & {
    faceBounds: { height: number };
    headerBounds: { width: number };
  },
): CSSProperties {
  return {
    ...audioNodeCardStyle(props, props.faceBounds.height),
    '--audio-node-header-actions-inset': `${props.headerBounds.width + spacing[3]}px`,
  } as CSSProperties;
}

function NodeHeaderActions({
  title,
  actions,
  mute,
  bypassControls,
  bypassDisabled,
  onBypassChange,
  headerRef,
  targetRef,
}: {
  title: string;
  actions?: ReactNode;
  mute?: ReactNode;
  bypassControls: NonNullable<AudioNodeCardProps['parameterControls']>;
  bypassDisabled: boolean;
  onBypassChange?: AudioNodeCardProps['onParameterChange'];
  headerRef: Ref<HTMLDivElement>;
  targetRef: Ref<HTMLDivElement>;
}) {
  const focus = useNodeHeaderFocus();
  return (
    <div className={styles.headerActions} ref={headerRef} {...focus}>
      <div className={styles.headerSlot}>{actions}</div>
      <div className={styles.headerSlot}>{mute}</div>
      <div className={styles.headerSlot}>
        <BuiltinBypass
          title={title}
          controls={bypassControls}
          disabled={bypassDisabled}
          onChange={onBypassChange}
        />
      </div>
      <div className={styles.headerSlot} ref={targetRef} />
    </div>
  );
}

function BuiltinBypass({
  title,
  controls,
  disabled,
  onChange,
}: {
  title: string;
  controls: NonNullable<AudioNodeCardProps['parameterControls']>;
  disabled: boolean;
  onChange: AudioNodeCardProps['onParameterChange'];
}) {
  const control = controls.find((item) => item.id === 'bypass');
  if (!control || !onChange) return null;
  const label = `${title}: ${control.label}: ${control.choices?.find((choice) => choice.value === control.value)?.label ?? control.value}`;
  return (
    <IconActionButton
      label={label}
      aria-pressed={Boolean(control.value)}
      disabled={disabled}
      onClick={() => onChange(control.id, control.value ? 0 : 1)}
    >
      <svg viewBox="0 0 20 20" aria-hidden="true">
        <path d="M2 10h4m8 0h4M6 6h8v8H6z" />
        {control.value ? <path d="m4 16 12-12" /> : null}
      </svg>
    </IconActionButton>
  );
}

function nodeOptionsAvailability(
  props: Pick<
    AudioNodeCardProps,
    | 'gainControl'
    | 'onGainChange'
    | 'endpointControl'
    | 'onEndpointChange'
    | 'channelLayoutControl'
    | 'onChannelLayoutChange'
    | 'parameterControls'
    | 'onParameterChange'
    | 'processorEditor'
  >,
) {
  const hasGainControl = Boolean(props.gainControl && props.onGainChange);
  const channelLayout =
    props.channelLayoutControl &&
    props.onChannelLayoutChange &&
    props.channelLayoutControl.options.length > 1
      ? { control: props.channelLayoutControl, onChange: props.onChannelLayoutChange }
      : undefined;
  const hasEndpointControl = Boolean(props.endpointControl && props.onEndpointChange);
  const hasParameterControls =
    Boolean(props.parameterControls?.length && props.onParameterChange) &&
    !prefersProcessorEditor(props.processorEditor);
  return {
    hasOptions:
      hasGainControl || hasEndpointControl || hasParameterControls || Boolean(channelLayout),
    channelLayout,
    hasEndpointControl,
    hasParameterControls,
  };
}

function AudioNodeControls({
  content,
  compactMeterBanks,
  dynamics,
  parameterControls,
  bypassed,
  faceMode,
  measureRef,
  offline,
  title,
  children,
}: {
  content: ReactNode;
  compactMeterBanks: readonly AudioNodeMeterBank[];
  dynamics?: DynamicsMeterContract;
  parameterControls: NonNullable<AudioNodeCardProps['parameterControls']>;
  bypassed: boolean;
  faceMode: AudioNodeFaceMode;
  measureRef: Ref<HTMLDivElement>;
  offline: boolean;
  title: string;
  children: ReactNode;
}) {
  // DOM content and settings set the minimum height; the graph owns native editor geometry.
  const compact = faceMode === 'compact';
  const dynamicsMeter = dynamics ? (
    <DynamicsMeter
      contract={dynamics}
      controls={parameterControls}
      disabled={offline}
      bypassed={bypassed}
    />
  ) : null;
  if (compact && compactMeterBanks.length > 0) {
    return (
      <div className={styles.controls} data-node-controls="">
        <div ref={measureRef} className={styles.settings}>
          <AudioNodeMeterBanks banks={compactMeterBanks} offline={offline} title={title} />
          {dynamicsMeter}
          {children}
          {content ? (
            <div className={`${styles.content} nodrag nopan nowheel`}>{content}</div>
          ) : null}
        </div>
      </div>
    );
  }
  return (
    <div ref={compact ? undefined : measureRef} className={styles.controls} data-node-controls="">
      {!compact && content ? (
        <div className={`${styles.content} nodrag nopan nowheel`}>{content}</div>
      ) : null}
      <div ref={compact ? measureRef : undefined} className={styles.settings}>
        {compact && content ? (
          <div className={`${styles.content} nodrag nopan nowheel`}>{content}</div>
        ) : null}
        {dynamicsMeter}
        {children}
      </div>
    </div>
  );
}

function audioNodeCardStyle(
  {
    faceHeight,
    faceMode,
    meterBanks,
  }: Pick<AudioNodeCardProps, 'faceHeight' | 'faceMode' | 'meterBanks'>,
  renderedFaceHeight: number,
): CSSProperties {
  const geometry = audioNodeFaceGeometry({
    meterBanks,
    mode: faceMode,
  });
  const height = Math.max(faceHeight ?? 0, geometry.height);
  return {
    '--audio-node-face-height': `${height}px`,
    '--audio-node-header-height': `${audioNodeFrame.headerHeight}px`,
    '--audio-node-utility-height': `${audioNodeFrame.utilityHeight}px`,
    '--audio-node-border-width': `${audioNodeFrame.borderWidth}px`,
    '--audio-node-rendered-face-height': `${renderedFaceHeight > 0 ? renderedFaceHeight : height}px`,
    // Meter rows set the channel pitch that cable anchors follow, so every face uses them.
    '--audio-node-body-padding': `${channelRowMetrics.bodyPadding}px`,
    '--audio-node-meter-margin': `${channelRowMetrics.meterMargin}px`,
    '--audio-node-row-height': `${channelRowMetrics.rowHeight}px`,
    '--audio-node-row-gap': `${channelRowMetrics.rowGap}px`,
  } as CSSProperties;
}

function AudioNodeFace(
  props: Pick<
    AudioNodeCardProps,
    | 'category'
    | 'processingTimeNs'
    | 'channelCount'
    | 'detail'
    | 'faceMode'
    | 'deviceAppActivity'
    | 'icon'
    | 'iconUrl'
    | 'inputCount'
    | 'meterBanks'
    | 'onSelect'
    | 'offline'
    | 'outputCount'
    | 'selected'
    | 'status'
    | 'statusTone'
    | 'title'
  > & { hideSummary?: boolean; faceRef?: (element: HTMLElement | null) => void },
) {
  const {
    deviceAppActivity,
    processingTimeNs,
    category,
    channelCount = 0,
    hideSummary,
    detail,
    icon,
    iconUrl,
    inputCount,
    meterBanks = [],
    faceMode = 'standard',
    onSelect,
    offline,
    outputCount,
    selected,
    status,
    statusTone,
    title,
  } = props;
  const Face = onSelect ? 'button' : 'div';
  return (
    <Face
      type={onSelect ? 'button' : undefined}
      ref={props.faceRef as Ref<HTMLButtonElement & HTMLDivElement>}
      data-node-face=""
      aria-label={
        onSelect ? [title, category, detail, status].filter(Boolean).join(', ') : undefined
      }
      aria-description={onSelect ? deviceAppActivity?.summary : undefined}
      aria-pressed={onSelect ? selected : undefined}
      className={styles.button}
      onClick={onSelect}
    >
      <span className={styles.topLine}>
        <span className={styles.icon} aria-hidden="true">
          <AudioNodeIcon icon={icon} iconUrl={iconUrl} />
        </span>
        <span className={styles.category} data-node-title>
          {title}
        </span>
        <ProcessorTimingLabel nanoseconds={processingTimeNs} />
        {status ? (
          <span className={styles.statusSlot}>
            <StatusIndicator label={status} tone={statusTone} position="left" />
          </span>
        ) : null}
      </span>
      <span className={styles.body}>
        {faceMode !== 'compact' && hideSummary && detail ? (
          <small className={styles.detail}>{detail}</small>
        ) : null}
        {faceMode !== 'compact' && meterBanks.length > 0 ? (
          <AudioNodeMeterBanks banks={meterBanks} offline={Boolean(offline)} title={title} />
        ) : null}
      </span>
      {!hideSummary ? (
        <NodeIoSummary {...{ inputCount, outputCount, channelCount, deviceAppActivity, detail }} />
      ) : null}
    </Face>
  );
}

function CompactNodeFeedback({
  faceMode,
  detail,
  status,
  statusTone,
}: Pick<AudioNodeCardProps, 'faceMode' | 'detail' | 'status' | 'statusTone'>) {
  if (faceMode !== 'compact') return null;
  const message = statusTone === 'critical' ? status || detail : detail;
  return message ? (
    <div className={styles.compactDetail}>
      <Text kind="label" tone={statusTone}>
        {message}
      </Text>
    </div>
  ) : null;
}

function NodeIoSummary({
  inputCount,
  outputCount,
  channelCount,
  deviceAppActivity,
  detail,
}: Pick<AudioNodeCardProps, 'inputCount' | 'outputCount' | 'deviceAppActivity' | 'detail'> & {
  channelCount: number;
}) {
  return (
    <span className={styles.ioSummary}>
      <span aria-hidden="true">
        {inputCount > 0 && outputCount > 0 && inputCount !== outputCount
          ? `IN ${inputCount} / OUT ${outputCount}`
          : `${Math.max(channelCount, inputCount, outputCount)} CH`}
      </span>
      {deviceAppActivity ? <DeviceAppActivity {...deviceAppActivity} /> : null}
      {detail ? <small className={styles.detail}>{detail}</small> : null}
    </span>
  );
}

interface NodeOptionsProps {
  disabled: boolean;
  endpointControl?: EndpointControl;
  channelLayout?: { control: NodeChoiceControl; onChange: (value: string) => void };
  gainControl?: { label: string; valueDb: number };
  hasOptions: boolean;
  label: string;
  onEndpointChange?: AudioNodeCardProps['onEndpointChange'];
  onGainChange?: AudioNodeCardProps['onGainChange'];
  onParameterChange?: AudioNodeCardProps['onParameterChange'];
  onToggle: () => void;
  opened: boolean;
  parameterControls: NonNullable<AudioNodeCardProps['parameterControls']>;
  equalizer?: GraphicEqualizerContract;
  onParametersChange?: AudioNodeCardProps['onParametersChange'];
}

function NodeOptions(props: NodeOptionsProps) {
  if (!props.hasOptions) return null;
  const layoutField = props.channelLayout ? (
    <SelectField
      density="compact"
      label={props.channelLayout.control.label}
      value={props.channelLayout.control.value}
      data={[...props.channelLayout.control.options]}
      disabled={props.disabled}
      onChange={props.channelLayout.onChange}
    />
  ) : null;
  return props.equalizer && props.onParameterChange ? (
    <EqualizerOptions
      {...props}
      equalizer={props.equalizer}
      onParameterChange={props.onParameterChange}
      layoutField={layoutField}
    />
  ) : (
    <StandardOptions {...props} layoutField={layoutField} />
  );
}

function EqualizerOptions({
  disabled,
  equalizer,
  label,
  layoutField,
  onParameterChange,
  onParametersChange,
  onToggle,
  opened,
  parameterControls,
}: NodeOptionsProps & {
  equalizer: GraphicEqualizerContract;
  onParameterChange: NonNullable<AudioNodeCardProps['onParameterChange']>;
  layoutField: ReactNode;
}) {
  return (
    <div className={`${styles.optionsPanel} nodrag nopan nowheel`}>
      <GraphicEqualizer
        contract={equalizer}
        controls={parameterControls}
        disabled={disabled}
        bypassed={Boolean(parameterControls.find((control) => control.id === 'bypass')?.value)}
        onChange={onParameterChange}
        onParametersChange={onParametersChange}
        renderValues={(values) => (
          <NodeOptionsSection label={label} opened={opened} onToggle={onToggle} disabled={disabled}>
            {layoutField}
            {values}
          </NodeOptionsSection>
        )}
      />
    </div>
  );
}

function StandardOptions(props: NodeOptionsProps & { layoutField: ReactNode }) {
  const mapping = useContext(MidiContext)?.active ?? false;
  const volume = useMidiControl('volume');
  return mapping ? (
    <MidiVolumeControl control={props.gainControl} available={Boolean(props.onGainChange)} />
  ) : (
    <StandardAudioOptions {...props} opened={props.opened || Boolean(volume?.selected)} />
  );
}

function StandardAudioOptions({
  disabled,
  endpointControl,
  gainControl,
  label,
  layoutField,
  onEndpointChange,
  onGainChange,
  onParameterChange,
  onToggle,
  opened,
  parameterControls,
}: NodeOptionsProps & { layoutField: ReactNode }) {
  const inline = parameterControls.length > 0;
  const mainParameters = parameterControls.filter(
    (control) => control.id !== 'bypass' && control.section !== 'options',
  );
  const optionParameters = onParameterChange
    ? parameterControls.filter((control) => control.section === 'options')
    : [];
  const controls = (
    <>
      {inline ? null : layoutField}
      {endpointControl && onEndpointChange ? (
        <div inert={disabled && !endpointControl.nativeFormats}>
          <AudioNodeEndpointOptions control={endpointControl} onChange={onEndpointChange} />
        </div>
      ) : null}
      {gainControl && onGainChange ? (
        <NodeGainControl control={gainControl} onChange={onGainChange} disabled={disabled} />
      ) : null}
      {onParameterChange && inline ? (
        <AudioNodeParameterGrid
          controls={mainParameters}
          disabled={disabled}
          onChange={onParameterChange}
        />
      ) : null}
    </>
  );
  if (!inline)
    return (
      <NodeOptionsSection label={label} opened={opened} onToggle={onToggle}>
        {controls}
      </NodeOptionsSection>
    );
  return (
    <>
      <div className={`${styles.optionsPanel} nodrag nopan nowheel`}>{controls}</div>
      {layoutField || optionParameters.length > 0 ? (
        <NodeOptionsSection label={label} opened={opened} onToggle={onToggle} disabled={disabled}>
          {layoutField}
          {onParameterChange && optionParameters.length > 0 ? (
            <AudioNodeParameterGrid
              controls={optionParameters}
              disabled={disabled}
              onChange={onParameterChange}
            />
          ) : null}
        </NodeOptionsSection>
      ) : null}
    </>
  );
}

function prefersProcessorEditor(control: AudioNodeCardProps['processorEditor']) {
  return Boolean(control && control.state !== 'failed' && control.state !== 'unavailable');
}

function ProcessorEditorOption({
  title,
  control,
  onRetry,
}: {
  title: string;
  control?: AudioNodeCardProps['processorEditor'];
  onRetry?: AudioNodeCardProps['onProcessorEditorRetry'];
}) {
  if (!control || !onRetry || (control.state !== 'failed' && control.state !== 'closed'))
    return null;
  return (
    <div className={`${styles.processorEditor} nodrag nopan nowheel`}>
      <Tooltip label={`${title}: ${control.actionLabel}`}>
        <Button
          aria-label={`${title}: ${control.actionLabel}`}
          className={styles.processorEditorButton}
          data-state={control.state}
          onClick={(event) => {
            event.stopPropagation();
            onRetry();
          }}
        >
          <svg
            aria-hidden="true"
            width="14"
            height="14"
            viewBox="0 0 16 16"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.25"
          >
            {control.state === 'closed' ? (
              <path d="M6 3H3v10h10v-3M9 3h4v4M13 3 7 9" />
            ) : (
              <path d="M13 7a5 5 0 1 0-1 4M13 2v5H8" />
            )}
          </svg>
        </Button>
      </Tooltip>
      {control.failureLabel ? (
        <span role="status" className={styles.processorEditorFailure}>
          {control.failureLabel}
        </span>
      ) : null}
    </div>
  );
}

function NodePortOverlays({
  inputCount,
  outputCount,
  visible,
}: {
  inputCount: number;
  outputCount: number;
  visible: boolean;
}) {
  if (!visible) return null;
  return (
    <>
      <NodePorts count={inputCount} side="input" />
      <NodePorts count={outputCount} side="output" />
    </>
  );
}

function MidiVolumeControl({
  control,
  available,
}: {
  control: NodeOptionsProps['gainControl'];
  available: boolean;
}) {
  const mapping = useMidiControl('volume');
  const { t } = useTranslation();
  if (!control || !available || !mapping) return null;
  return (
    <ActionButton
      density="compact"
      data-midi-map
      aria-label={mapping.label}
      aria-pressed={mapping.selected}
      onClick={mapping.select}
      onKeyDown={mapping.keyDown}
    >
      {mapping.text || t('midi.actions.volume')}
    </ActionButton>
  );
}

function NodeGainControl({
  control,
  onChange,
  disabled,
}: {
  control: { label: string; valueDb: number };
  onChange: (gainDb: number) => void;
  disabled: boolean;
}) {
  const mapping = useMidiControl('volume');
  const [value, setValue] = useState(control.valueDb);
  useEffect(() => setValue(control.valueDb), [control.valueDb]);
  return (
    <span
      className={`${styles.gainControl} nodrag nopan nowheel`}
      data-midi-selected={mapping?.selected || undefined}
    >
      <span className={styles.gainValue}>{value.toFixed(1)} dB</span>
      <ValueSlider
        disabled={disabled}
        max={12}
        min={-60}
        onChange={(nextValue) => {
          setValue(nextValue);
          onChange(nextValue);
        }}
        step={0.5}
        label={control.label}
        value={value}
        valueText={`${value} dB`}
      />
    </span>
  );
}

function NodePorts({ count, side }: { count: number; side: 'input' | 'output' }) {
  return (
    <span
      aria-hidden="true"
      className={`${styles.ports} ${side === 'input' ? styles.inputPorts : styles.outputPorts}`}
    >
      {portIds.slice(0, count).map((portId, portIndex) => (
        <span className={styles.port} data-port-index={portIndex} key={`${side}-${portId}`} />
      ))}
    </span>
  );
}
