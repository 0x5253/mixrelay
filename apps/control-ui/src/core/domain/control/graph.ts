export interface RuntimeGraphNode {
  control?: NodeControlState;
  config: unknown;
  id: string;
  label: string | null;
  type: string;
}

export interface NodeControlState {
  enabled: boolean;
  muted: boolean;
}

export interface NodeControlRequest {
  clientOperationId?: string;
  expectedRevision: number;
  enabled?: boolean;
  muted?: boolean;
  gainDb?: number;
}

export interface NodeControlResponse extends GraphApplyResponse {
  /** Native preparation may reconcile authored metadata outside the requested fields. */
  requiresGraphReload: boolean;
}

export interface GraphNodeControls {
  graphId: string;
  revision: number;
  nodes: {
    nodeId: string;
    label: string | null;
    kind: string;
    control: NodeControlState;
    capabilities: { power: boolean; mute: boolean; recorder: boolean };
    gainDb?: number;
  }[];
}

export interface RuntimeAudioEndpoint {
  nodeId: string;
  portId: string;
}

export interface RuntimeChannelRoute {
  destinationChannel: number;
  sourceChannel: number;
}

export interface RuntimeGraphEdge {
  channelMapping: { mode: 'auto' } | { mode: 'explicit'; routes: RuntimeChannelRoute[] };
  destination: RuntimeAudioEndpoint;
  id: string;
  source: RuntimeAudioEndpoint;
}

export interface RuntimeGraphDocument {
  edges: RuntimeGraphEdge[];
  nodes: RuntimeGraphNode[];
  revision: number;
  schemaVersion: 2;
}

export interface GraphRuntimeStatus {
  pendingReason?: 'awaiting_microphone_permission';
  activeRevision: number;
  protocolVersion: 1;
  submittedRevision: number;
}

export interface GraphApplyResponse {
  protocolVersion: 1;
  revision: number;
  state: 'active' | 'pending';
  persistenceError?: string;
}

export interface ProcessorRetryRequest {
  clientOperationId?: string;
  expectedCatalogRevision: number;
  expectedInstanceId: string;
  expectedRevision: number;
  graphId: string;
  nodeId: string;
}

/** Only controls explicitly edited by this client; never inferred from a stale engine snapshot. */
export interface ProcessorParameterChange {
  nodeId: string;
  typeId: string;
  parameterId: number;
  value: number;
}

export interface GraphApplyRequest extends RuntimeGraphDocument {
  clientOperationId?: string;
  processorParameterChanges?: ProcessorParameterChange[];
  /** The graph this document was authored for; the engine rejects it once another is selected. */
  expectedGraphId?: string;
}
