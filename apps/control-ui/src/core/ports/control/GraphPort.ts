import type {
  GraphApplyRequest,
  GraphApplyResponse,
  GraphNodeControls,
  GraphRuntimeStatus,
  NodeControlRequest,
  NodeControlResponse,
  ProcessorRetryRequest,
  RuntimeGraphDocument,
} from '../../domain/control/graph';
import type {
  AudioSessionStatus,
  MixRelayStatus,
  VirtualEndpointFormatRequest,
} from '../../domain/control/status';

/** The active audio graph, its runtime status and engine-wide audio settings. */
export interface GraphPort {
  applyGraph(document: GraphApplyRequest): Promise<GraphApplyResponse>;
  getGraph(): Promise<RuntimeGraphDocument>;
  getGraphStatus(): Promise<GraphRuntimeStatus>;
  getStatus(): Promise<MixRelayStatus>;
  isOperationPending?(operationId: string): Promise<boolean>;
  getNodeControls(graphId: string): Promise<GraphNodeControls>;
  updateNodeControl(
    graphId: string,
    nodeId: string,
    request: NodeControlRequest,
  ): Promise<NodeControlResponse>;
  /** Explicitly retries one failed processor instance; adapters must not retry this request. */
  retryProcessor(request: ProcessorRetryRequest): Promise<GraphApplyResponse>;
  setEnginePower?(enabled: boolean): Promise<GraphApplyResponse>;
  reconfigureSession?(request: {
    expectedRevision: number;
    format: AudioSessionStatus['format'];
  }): Promise<GraphApplyResponse>;
  setVirtualEndpointSampleRate?(request: VirtualEndpointFormatRequest): Promise<GraphApplyResponse>;
}
