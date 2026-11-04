import { createContext, type ReactNode, useContext, useMemo } from 'react';
import { AudioMeterProvider } from '../audio-meter/AudioMeterProvider';
import type { AudioMeterStore } from '../audio-meter/AudioMeterStore';
import { NodeOptionsContext, type NodeOptionsState } from '../ui/nodeOptionsState';
import type { AudioGraphCanvasProps, AudioNodeControlSlots } from './AudioGraphCanvas';
import type { AudioGraphModel, AudioGraphNode } from './model';
import type { NodeProcessorEditors } from './nodeViewModel';
import type { SelectedAudioPort } from './portSelection';

/** Per-node edits a node card reports from its controls and Options. */
export type NodeEdits = Pick<
  AudioGraphCanvasProps,
  | 'onNodeEndpointChange'
  | 'onNodeChannelLayoutChange'
  | 'onNodeGainChange'
  | 'onNodeParameterChange'
  | 'onNodeParametersChange'
>;

interface NodeCallbacks {
  gain?: NodeEdits['onNodeGainChange'];
  /** Node settings edited from Options. */
  endpoint?: NodeEdits['onNodeEndpointChange'];
  channelLayout?: NodeEdits['onNodeChannelLayoutChange'];
  parameter?: NodeEdits['onNodeParameterChange'];
  parameters?: NodeEdits['onNodeParametersChange'];
  processorEditors: NodeProcessorEditors | null;
  renderContent?: (node: AudioGraphNode) => ReactNode;
  renderControls?: (node: AudioGraphNode) => AudioNodeControlSlots | null;
}

interface AudioPortSelectionContextValue {
  model: AudioGraphModel;
  selectedRef: { current: SelectedAudioPort[] };
}

/** One memoized value so nodes skip renders when the canvas re-renders. */
const NodeCallbacksContext = createContext<NodeCallbacks | null>(null);
const AudioPortSelectionContext = createContext<AudioPortSelectionContextValue | null>(null);
const NodeDisclosureHeightsContext = createContext<Map<string, number> | null>(null);

// The readers below are private to renderers installed only inside AudioGraphProviders.
export function useNodeCallbacks() {
  return useContext(NodeCallbacksContext) as NodeCallbacks;
}

export function useAudioPortSelectionContext() {
  return useContext(AudioPortSelectionContext) as AudioPortSelectionContextValue;
}

export function useNodeDisclosureHeights() {
  return useContext(NodeDisclosureHeightsContext) as Map<string, number>;
}

export function AudioGraphProviders({
  children,
  options,
  editor,
  meterStore,
  model,
  edits,
  selectedRef,
  renderNodeContent,
  renderNodeControls,
  disclosureHeights,
}: {
  children: ReactNode;
  options: NodeOptionsState;
  editor: NodeProcessorEditors | null;
  meterStore: AudioMeterStore | null;
  model: AudioGraphModel;
  edits: NodeEdits;
  selectedRef: AudioPortSelectionContextValue['selectedRef'];
  renderNodeContent: AudioGraphCanvasProps['renderNodeContent'];
  renderNodeControls: AudioGraphCanvasProps['renderNodeControls'];
  disclosureHeights: Map<string, number>;
}) {
  // Context values are memoized: a new object would re-render every node.
  const {
    onNodeEndpointChange,
    onNodeChannelLayoutChange,
    onNodeGainChange,
    onNodeParameterChange,
    onNodeParametersChange,
  } = edits;
  const callbacks = useMemo<NodeCallbacks>(
    () => ({
      gain: onNodeGainChange,
      endpoint: onNodeEndpointChange,
      channelLayout: onNodeChannelLayoutChange,
      parameter: onNodeParameterChange,
      parameters: onNodeParametersChange,
      processorEditors: editor,
      renderContent: renderNodeContent,
      renderControls: renderNodeControls,
    }),
    [
      editor,
      onNodeChannelLayoutChange,
      onNodeEndpointChange,
      onNodeGainChange,
      onNodeParameterChange,
      onNodeParametersChange,
      renderNodeContent,
      renderNodeControls,
    ],
  );
  const selection = useMemo(() => ({ model, selectedRef }), [model, selectedRef]);
  // `useState` returns a new tuple every render; keep the context value stable.
  const [openedOptions, setOpenedOptions] = options;
  const optionsValue = useMemo<NodeOptionsState>(
    () => [openedOptions, setOpenedOptions],
    [openedOptions, setOpenedOptions],
  );
  return (
    <AudioMeterProvider store={meterStore}>
      <NodeCallbacksContext.Provider value={callbacks}>
        <AudioPortSelectionContext.Provider value={selection}>
          <NodeOptionsContext.Provider value={optionsValue}>
            <NodeDisclosureHeightsContext.Provider value={disclosureHeights}>
              {children}
            </NodeDisclosureHeightsContext.Provider>
          </NodeOptionsContext.Provider>
        </AudioPortSelectionContext.Provider>
      </NodeCallbacksContext.Provider>
    </AudioMeterProvider>
  );
}
