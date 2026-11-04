import { type ReactNode, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useMidiMappingActive } from '../midi/MidiContext';
import styles from './AudioNodeCard.module.css';
import { AudioNodeControls, ProcessorEditorOption } from './AudioNodeControls';
import type { EndpointControl, EndpointUpdate } from './AudioNodeEndpointOptions';
import { AudioNodeFace, CompactNodeFeedback } from './AudioNodeFace';
import { AudioNodeHeaderActions } from './AudioNodeHeaderActions';
import type { AudioNodeMeterBank } from './AudioNodeMeterBanks';
import { NodeOptions } from './AudioNodeOptions';
import type { AudioNodeParameterControl } from './AudioNodeParameterGrid';
import { AudioNodePorts } from './AudioNodePorts';
import {
  audioNodeCardStyle,
  cardStateAttributes,
  meterBanksForCompactFace,
  nodeOptionsAvailability,
  type ProcessorEditorControl,
} from './audioNodeCardModel';
import type { AudioNodeFaceMode } from './audioNodeLayout';
import type { DeviceAppActivityProps } from './DeviceAppActivity';
import type { DynamicsMeterContract } from './DynamicsMeter';
import type { GraphicEqualizerContract } from './GraphicEqualizer';
import { NodeDisclosureMeasureContext, NodeHeaderOptionsContext } from './NodeOptionsSection';
import { useNodeOptionsState } from './nodeOptionsState';
import type { StatusTone } from './StatusBadge';
import { useCardMeasurements } from './useAudioNodeCardMeasurements';
import { useElementBounds } from './useElementBounds';

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
  processorEditor?: ProcessorEditorControl;
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
  const mapping = useMidiMappingActive();
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
      style={audioNodeCardStyle({
        faceHeight,
        faceMode,
        meterBanks,
        renderedFaceHeight: faceBounds.height,
        headerActionsWidth: headerBounds.width,
      })}
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
          <AudioNodeHeaderActions
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
          <AudioNodePorts inputCount={inputCount} outputCount={outputCount} visible={showPorts} />
        </NodeHeaderOptionsContext.Provider>
      </NodeDisclosureMeasureContext.Provider>
    </div>
  );
}
