import type { CSSProperties } from 'react';
import { spacing } from '../theme/tokens';
import type { AudioNodeCardProps, NodeChoiceControl } from './AudioNodeCard';
import type { AudioNodeMeterBank } from './AudioNodeMeterBanks';
import type { AudioNodeParameterControl } from './AudioNodeParameterGrid';
import {
  type AudioNodeFaceMode,
  audioNodeFaceGeometry,
  audioNodeFrame,
  channelRowMetrics,
} from './audioNodeLayout';
import type { StatusTone } from './StatusBadge';

/** A native plug-in editor window a processor node can open. */
export interface ProcessorEditorControl {
  actionLabel: string;
  failureLabel?: string;
  state: 'unavailable' | 'closed' | 'opening' | 'open' | 'failed';
}

/** A working native editor replaces the generic parameter list in Options. */
export function prefersProcessorEditor(control: ProcessorEditorControl | undefined) {
  return Boolean(control && control.state !== 'failed' && control.state !== 'unavailable');
}

/** The editor action is offered to open a closed editor or retry a failed one. */
export function offersProcessorEditorAction(control: ProcessorEditorControl) {
  return control.state === 'failed' || control.state === 'closed';
}

/** Which settings the Options panel can show; a setting needs both a value and an owner. */
export function nodeOptionsAvailability(
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
  const channelLayout: ChannelLayoutChoice | undefined =
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

export interface ChannelLayoutChoice {
  control: NodeChoiceControl;
  onChange: (value: string) => void;
}

/** Parameters shown on the card face, and those kept in the Options disclosure. Bypass
 * belongs to the header. Options parameters need an owner to edit them. */
export function partitionParameterControls(
  controls: readonly AudioNodeParameterControl[],
  editable: boolean,
) {
  return {
    main: controls.filter((control) => control.id !== 'bypass' && control.section !== 'options'),
    options: editable ? controls.filter((control) => control.section === 'options') : [],
  };
}

export function bypassControl(controls: readonly AudioNodeParameterControl[]) {
  return controls.find((control) => control.id === 'bypass');
}

/** The bypass button names the node, the parameter and its current choice. */
export function bypassLabel(title: string, control: AudioNodeParameterControl) {
  const choice = control.choices?.find((item) => item.value === control.value)?.label;
  return `${title}: ${control.label}: ${choice ?? control.value}`;
}

/** Channel summary: IN/OUT when the directions differ, otherwise the widest count. */
export function ioSummaryText(inputCount: number, outputCount: number, channelCount: number) {
  return inputCount > 0 && outputCount > 0 && inputCount !== outputCount
    ? `IN ${inputCount} / OUT ${outputCount}`
    : `${Math.max(channelCount, inputCount, outputCount)} CH`;
}

/** A compact face shows its detail, or the status instead when the status is critical. */
export function compactFeedbackMessage(
  faceMode: AudioNodeFaceMode,
  detail: string | undefined,
  status: string | undefined,
  statusTone: StatusTone | undefined,
) {
  if (faceMode !== 'compact') return undefined;
  return statusTone === 'critical' ? status || detail : detail;
}

/** Compact faces move their meters into the controls area. */
export function meterBanksForCompactFace(
  faceMode: AudioNodeFaceMode,
  meterBanks: readonly AudioNodeMeterBank[],
) {
  return faceMode === 'compact' ? meterBanks : [];
}

export function cardStateAttributes({
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

/** The card's geometry as custom properties; cable anchors follow the same metrics. */
export function audioNodeCardStyle({
  faceHeight,
  faceMode,
  meterBanks,
  renderedFaceHeight,
  headerActionsWidth,
}: Pick<AudioNodeCardProps, 'faceHeight' | 'faceMode' | 'meterBanks'> & {
  renderedFaceHeight: number;
  headerActionsWidth: number;
}): CSSProperties {
  const geometry = audioNodeFaceGeometry({ meterBanks, mode: faceMode });
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
    '--audio-node-header-actions-inset': `${headerActionsWidth + spacing[3]}px`,
  } as CSSProperties;
}
