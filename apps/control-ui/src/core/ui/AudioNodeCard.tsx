import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useMidiMappingActive } from '../midi/MidiContext';
import styles from './AudioNodeCard.module.css';
import { AudioNodeControls, ProcessorEditorOption } from './AudioNodeControls';
import { AudioNodeFace, CompactNodeFeedback } from './AudioNodeFace';
import { AudioNodeHeaderActions } from './AudioNodeHeaderActions';
import { NodeOptions } from './AudioNodeOptions';
import { AudioNodePorts } from './AudioNodePorts';
import {
  audioNodeCardStyle,
  cardStateAttributes,
  meterBanksForCompactFace,
  nodeOptionsAvailability,
} from './audioNodeCardModel';
import type { AudioNodeCardProps } from './audioNodeCardTypes';
import { NodeDisclosureMeasureContext, NodeHeaderOptionsContext } from './NodeOptionsSection';
import { useNodeOptionsState } from './nodeOptionsState';
import { useCardMeasurements } from './useAudioNodeCardMeasurements';
import { useElementBounds } from './useElementBounds';

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
