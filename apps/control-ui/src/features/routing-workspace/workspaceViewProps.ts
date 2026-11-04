import type { ReactNode } from 'react';
import type { GraphApplicationQueue } from '../../core/application/workspace/GraphApplicationQueue';
import type { AudioGraphChangeKind } from '../../core/audio-graph/audioGraphCanvasTypes';
import type { AudioGraphModel } from '../../core/audio-graph/model';
import type { PlaceGraphNodes } from '../../core/audio-graph/nodePlacement';
import type {
  AudioDeviceDescriptor,
  CapturableApplication,
} from '../../core/domain/control/inventory';
import type { ExternalProcessorDescriptor } from '../../core/domain/control/processors';
import type {
  AudioSessionStatus,
  FailedProcessorInstance,
  ProcessorTiming,
} from '../../core/domain/control/status';
import type { ControlPort } from '../../core/ports/control/ControlPort';
import type { RecorderNodeInputs } from '../../core/ports/control/RecorderNodeView';
import type { DeviceDriverAction } from './driverEndpointControls';
import type { useGraphLayout } from './useGraphLayout';
import type { useProcessorCatalog } from './useProcessorCatalog';
import type { useProcessorParameterPublisher } from './useProcessorParameterPublisher';
import type { useRoutingInventory } from './useRoutingInventory';
import type { VirtualEndpointUpdate } from './virtualEndpointNodes';
import type { WorkspaceProblemSummary, WorkspaceState } from './WorkspaceProblems';

export type UpdateModel = (model: AudioGraphModel, kind: AudioGraphChangeKind) => void;

/** Everything the workspace view renders once the graph, its edits and inventory are owned. */
export interface WorkspaceViewProps {
  renderRecorder?: (input: RecorderNodeInputs) => ReactNode;
  permissionWaiting: boolean;
  documentRevision: number;
  headerActions?: ReactNode;
  headerEnd?: ReactNode;
  notice?: ReactNode;
  problemsTarget?: HTMLElement | null;
  onProblemsChange?: (summary: WorkspaceProblemSummary) => void;
  failedProcessors?: FailedProcessorInstance[];
  catalogRevision?: number;
  processorTimings?: ProcessorTiming[];
  graphId?: string;
  interactionDisabled: boolean;
  parameters: ReturnType<typeof useProcessorParameterPublisher>;
  deviceDriverAction?: DeviceDriverAction;
  processorCatalog: ReturnType<typeof useProcessorCatalog>;
  layout: ReturnType<typeof useGraphLayout>;
  persistenceError: string | null;
  inventory: ReturnType<typeof useRoutingInventory>;
  onNodeMenuOpenedChange: (opened: boolean) => void;
  application: GraphApplicationQueue<AudioGraphModel>;
  audioSession?: AudioSessionStatus;
  playbackPaused?: boolean;
  addNode: (kind: string, position: { x: number; y: number }, placeNodes?: PlaceGraphNodes) => void;
  applications: readonly CapturableApplication[];
  audioDevices: readonly AudioDeviceDescriptor[];
  controlPort: ControlPort;
  errorCode: string | null;
  model: AudioGraphModel | null;
  processors: readonly ExternalProcessorDescriptor[];
  onTitleChange?: (title: string) => void;
  state: WorkspaceState;
  title?: string;
  updateModel: UpdateModel;
  updateEndpoint?: (nodeId: string, update: VirtualEndpointUpdate) => void;
  updateChannelLayout?: (nodeId: string, layoutId: string) => void;
  updateNodeGain?: (nodeId: string, gainDb: number) => void;
  updateNodeParameter?: (nodeId: string, parameterId: string, value: number) => void;
  updateNodeParameters?: (
    nodeId: string,
    updates: readonly { id: string; value: number }[],
  ) => void;
}
