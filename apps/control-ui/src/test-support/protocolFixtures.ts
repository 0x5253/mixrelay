import type { GraphCatalog, GraphLayoutResponse } from '../core/domain/control/catalog';
import type {
  GraphApplyResponse,
  GraphNodeControls,
  GraphRuntimeStatus,
  NodeControlResponse,
} from '../core/domain/control/graph';
import type {
  ProcessorEditorStatus,
  ProcessorParameterUpdateResponse,
} from '../core/domain/control/processors';
import type { RecorderStatus } from '../core/domain/control/recorder';
import type { MixRelayStatus } from '../core/domain/control/status';

/** Minimal replies that satisfy the engine protocol, for adapter tests that care about
 * routing rather than content. Spread overrides to vary one field. */
export const protocolFixtures = {
  status: (overrides: Partial<MixRelayStatus> = {}): MixRelayStatus => ({
    protocolVersion: 1,
    driverState: 'ready',
    sampleRate: 48000,
    bufferFrames: 128,
    underruns: 0,
    overruns: 0,
    ...overrides,
  }),
  graphStatus: (activeRevision = 2, submittedRevision = activeRevision): GraphRuntimeStatus => ({
    protocolVersion: 1,
    activeRevision,
    submittedRevision,
  }),
  apply: (revision: number, state: GraphApplyResponse['state'] = 'active'): GraphApplyResponse => ({
    protocolVersion: 1,
    revision,
    state,
  }),
  nodeControl: (revision: number, requiresGraphReload = false): NodeControlResponse => ({
    protocolVersion: 1,
    revision,
    state: 'active',
    requiresGraphReload,
  }),
  nodeControls: (graphId: string, revision = 2): GraphNodeControls => ({
    graphId,
    revision,
    nodes: [],
  }),
  layout: (graphId: string, layoutRevision = 0): GraphLayoutResponse => ({
    graphId,
    layoutRevision,
    layout: { positions: {} },
  }),
  catalog: (activeId = 'graph-1', catalogRevision = 1): GraphCatalog => ({
    protocolVersion: 1,
    catalogRevision,
    activeId,
    graphs: [{ id: activeId, name: 'Graph' }],
    document: { schemaVersion: 2, revision: 2, nodes: [], edges: [] },
  }),
  recorder: (overrides: Partial<RecorderStatus> = {}): RecorderStatus => ({
    state: 'stopped',
    path: null,
    sampleRate: null,
    encoding: null,
    frames: 0,
    error: null,
    ...overrides,
  }),
  editor: (
    nodeId: string,
    state: ProcessorEditorStatus['state'] = 'closed',
  ): ProcessorEditorStatus => ({
    protocolVersion: 1,
    nodeId,
    state,
    failure: null,
    width: null,
    height: null,
    resizable: false,
  }),
  parameter: (
    nodeId: string,
    parameterId: number,
    value: number,
  ): ProcessorParameterUpdateResponse => ({ protocolVersion: 1, nodeId, parameterId, value }),
};
