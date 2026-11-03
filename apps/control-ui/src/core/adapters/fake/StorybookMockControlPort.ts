import type {
  GraphCatalog,
  GraphCatalogRequest,
  GraphLayout,
  GraphLayoutRequest,
  GraphLayoutResponse,
} from '../../domain/control/catalog';
import { ControlPortError } from '../../domain/control/errors';
import type {
  GraphApplyRequest,
  GraphApplyResponse,
  GraphNodeControls,
  GraphRuntimeStatus,
  NodeControlRequest,
  NodeControlResponse,
  ProcessorRetryRequest,
  RuntimeGraphDocument,
  RuntimeGraphNode,
} from '../../domain/control/graph';
import type {
  AudioDeviceDescriptor,
  CapturableApplication,
  DeviceAppActivity,
} from '../../domain/control/inventory';
import type { MeterSnapshot } from '../../domain/control/meters';
import type {
  ExternalProcessorCatalog,
  ProcessorEditorFrame,
  ProcessorEditorInput,
  ProcessorEditorStatus,
  ProcessorParameterUpdateResponse,
} from '../../domain/control/processors';
import type {
  RecorderRequest,
  RecorderSettings,
  RecorderStatus,
} from '../../domain/control/recorder';
import type { MixRelayStatus } from '../../domain/control/status';
import type { ControlPort } from '../../ports/control/ControlPort';

/** The engine's revision conflict: a 409 the graph writer recognises as a definite rejection. */
function staleRevision(parameters: Record<string, unknown>) {
  return new ControlPortError(409, { code: 'graph_revision_stale', parameters });
}

function nodeGain(node: RuntimeGraphNode): number | undefined {
  if (
    !['gain', 'virtual_endpoint', 'audio_device_input', 'application_capture'].includes(node.type)
  )
    return undefined;
  const config = node.config as { gainDb?: number } | null;
  return config?.gainDb ?? 0;
}

export interface StorybookControlState {
  deviceAppActivity?: Record<string, DeviceAppActivity[]>;
  layout?: GraphLayout;
  applications?: CapturableApplication[];
  audioDevices?: AudioDeviceDescriptor[];
  graph: RuntimeGraphDocument;
  graphStatus: GraphRuntimeStatus;
  meters?: MeterSnapshot;
  processors?: ExternalProcessorCatalog;
  status: MixRelayStatus;
}

interface PreviewGraph {
  id: string;
  name: string;
  document: RuntimeGraphDocument;
}

export class StorybookMockControlPort implements ControlPort {
  private readonly recorderSettings = new Map<string, RecorderSettings>();

  public async recordingSettings(graphId: string, nodeId: string): Promise<RecorderSettings> {
    return {
      ...(this.recorderSettings.get(JSON.stringify([graphId, nodeId])) ?? {
        folder: '/Desktop',
        sampleRate: null,
        encoding: 'pcm16',
      }),
    };
  }

  public async setRecordingSettings(
    graphId: string,
    nodeId: string,
    settings: RecorderSettings,
    _expectedRevision: number,
  ): Promise<RecorderSettings> {
    this.recorderSettings.set(JSON.stringify([graphId, nodeId]), { ...settings });
    return { ...settings };
  }
  public async getNodeControls(graphId: string): Promise<GraphNodeControls> {
    const document =
      graphId === this.activeGraph.id ? this.state.graph : this.savedGraphs.get(graphId)?.document;
    if (!document) throw new Error('graph_invalid');
    return {
      graphId,
      revision: document.revision,
      nodes: document.nodes.map((node) => ({
        nodeId: node.id,
        label: node.label,
        kind: node.type,
        gainDb: nodeGain(node),
        control: node.control ?? { enabled: true, muted: false },
        capabilities: { power: true, mute: true, recorder: node.type === 'recorder' },
      })),
    };
  }
  public async updateNodeControl(
    graphId: string,
    nodeId: string,
    request: NodeControlRequest,
  ): Promise<NodeControlResponse> {
    const target = this.state.graph.nodes.find((node) => node.id === nodeId);
    if (
      graphId !== this.activeGraph.id ||
      request.expectedRevision !== this.state.graph.revision ||
      !target
    )
      throw staleRevision({ graphId, nodeId, expectedRevision: request.expectedRevision });
    if (
      request.gainDb !== undefined &&
      (nodeGain(target) === undefined ||
        !Number.isFinite(request.gainDb) ||
        request.gainDb < -60 ||
        request.gainDb > 12)
    )
      throw new Error('graph_invalid');
    const application = await this.applyGraph({
      ...this.state.graph,
      revision: request.expectedRevision + 1,
      nodes: this.state.graph.nodes.map((node) =>
        node.id === nodeId
          ? {
              ...node,
              config:
                request.gainDb === undefined
                  ? node.config
                  : { ...(node.config as object), gainDb: request.gainDb },
              control: {
                enabled: request.enabled ?? node.control?.enabled ?? true,
                muted: request.muted ?? node.control?.muted ?? false,
              },
            }
          : node,
      ),
    });
    return { ...application, requiresGraphReload: false };
  }
  public async recorderStatus(_graphId: string, _nodeId: string): Promise<RecorderStatus> {
    return {
      state: 'stopped',
      path: null,
      sampleRate: null,
      encoding: null,
      frames: 0,
      error: null,
    };
  }

  public async controlRecorder(
    _graphId: string,
    _nodeId: string,
    _request: RecorderRequest,
  ): Promise<RecorderStatus> {
    // The preview cannot write WAV files; never present simulated recording as a saved take.
    throw new Error('Recording requires the local audio engine.');
  }

  private readonly layouts = new Map<string, GraphLayoutResponse>();

  public async getGraphLayout(graphId: string): Promise<GraphLayoutResponse> {
    if (!this.savedGraphs.has(graphId)) throw new Error('graph_invalid');
    return structuredClone(
      this.layouts.get(graphId) ?? {
        graphId,
        layoutRevision: 0,
        layout: { positions: {} },
      },
    );
  }

  public async setGraphLayout(
    graphId: string,
    request: GraphLayoutRequest,
  ): Promise<GraphLayoutResponse> {
    const saved = await this.getGraphLayout(graphId);
    if (request.expectedLayoutRevision !== saved.layoutRevision)
      throw staleRevision({ resource: 'graph_layout', layoutRevision: saved.layoutRevision });
    const next = {
      graphId,
      layoutRevision: saved.layoutRevision + 1,
      layout: structuredClone(request.layout),
    };
    this.layouts.set(graphId, next);
    return structuredClone(next);
  }
  private readonly editorStates = new Map<string, ProcessorEditorStatus>();
  private catalogRevision = 1;
  private activeGraph: PreviewGraph;
  private readonly savedGraphs = new Map<string, PreviewGraph>();

  public constructor(private readonly state: StorybookControlState) {
    this.activeGraph = this.addCatalogGraph('graph-1', 'Untitled Graph', state.graph);
    if (state.layout)
      this.layouts.set('graph-1', {
        graphId: 'graph-1',
        layoutRevision: 0,
        layout: structuredClone(state.layout),
      });
  }

  private addCatalogGraph(id: string, name: string, document: RuntimeGraphDocument): PreviewGraph {
    const graph = { id, name, document: structuredClone(document) };
    this.savedGraphs.set(id, graph);
    return graph;
  }

  public async getGraphCatalog(): Promise<GraphCatalog> {
    return structuredClone({
      protocolVersion: 1,
      catalogRevision: this.catalogRevision,
      activeId: this.activeGraph.id,
      graphs: Array.from(this.savedGraphs, ([id, value]) => ({ id, name: value.name })),
      document: this.state.graph,
    });
  }

  private deletedGraphs: { graph: PreviewGraph; replacement?: string }[] = [];
  private restoredGraphs: PreviewGraph[] = [];

  public async mutateGraphCatalog(request: GraphCatalogRequest): Promise<GraphCatalog> {
    if (
      request.expectedRevision !== this.state.graph.revision ||
      request.expectedCatalogRevision !== this.catalogRevision
    ) {
      throw staleRevision({ catalogRevision: this.catalogRevision });
    }
    if (request.action === 'undo_delete' || request.action === 'redo_delete') {
      if (!(await this.restoreCatalogDeletion(request.action))) return this.getGraphCatalog();
    } else if (request.action === 'delete') {
      this.rememberCatalogDeletion(request.id);
      this.restoredGraphs = [];
      await this.removeCatalogGraph(request.id);
    } else if (request.action === 'rename') {
      const graph = this.savedGraphs.get(request.id);
      if (!graph) throw new Error('graph_invalid');
      graph.name = request.name;
    } else {
      this.activeGraph.document = structuredClone(this.state.graph);
      const target =
        request.action === 'create'
          ? this.addCatalogGraph(`graph-${this.catalogRevision + 1}`, request.name, {
              schemaVersion: 2,
              revision: 0,
              nodes: [],
              edges: [],
            })
          : this.savedGraphs.get(request.id);
      if (!target) throw new Error('graph_invalid');
      this.activeGraph = target;
      await this.applyGraph({
        ...structuredClone(target.document),
        revision: this.state.graph.revision + 1,
      });
    }
    this.catalogRevision += 1;
    return this.getGraphCatalog();
  }

  private rememberCatalogDeletion(id: string) {
    const graph = this.savedGraphs.get(id);
    if (graph)
      this.deletedGraphs.push({
        graph: structuredClone(graph),
        replacement: this.savedGraphs.size === 1 ? `graph-${this.catalogRevision + 1}` : undefined,
      });
  }

  private async restoreCatalogDeletion(action: 'undo_delete' | 'redo_delete') {
    if (action === 'undo_delete') {
      const removed = this.deletedGraphs.pop();
      if (!removed) return false;
      const graph = removed.graph;
      const replacement = removed.replacement && this.savedGraphs.get(removed.replacement);
      if (
        replacement &&
        replacement.document.nodes.length === 0 &&
        replacement.document.edges.length === 0 &&
        replacement.name === 'Untitled Graph'
      )
        this.savedGraphs.delete(replacement.id);
      this.savedGraphs.set(graph.id, graph);
      this.activeGraph = graph;
      await this.applyGraph({
        ...structuredClone(graph.document),
        revision: this.state.graph.revision + 1,
      });
      this.restoredGraphs.push(graph);
    } else {
      const graph = this.restoredGraphs.pop();
      if (!graph) return false;
      this.rememberCatalogDeletion(graph.id);
      await this.removeCatalogGraph(graph.id);
    }
    return true;
  }

  private async removeCatalogGraph(id: string) {
    const graphs = [...this.savedGraphs.values()];
    const index = graphs.findIndex((graph) => graph.id === id);
    if (index < 0) throw new Error('graph_invalid');
    this.savedGraphs.delete(id);
    graphs.splice(index, 1);
    if (this.activeGraph.id === id) {
      this.activeGraph =
        graphs[Math.min(index, graphs.length - 1)] ??
        this.addCatalogGraph(`graph-${this.catalogRevision + 1}`, 'Untitled Graph', {
          schemaVersion: 2,
          revision: 0,
          nodes: [],
          edges: [],
        });
    }
    await this.applyGraph({
      ...structuredClone(this.activeGraph.document),
      revision: this.state.graph.revision + 1,
    });
  }

  public async applyGraph(request: GraphApplyRequest): Promise<GraphApplyResponse> {
    const { expectedGraphId, ...intent } = request;
    if (expectedGraphId !== undefined && expectedGraphId !== this.activeGraph.id)
      throw staleRevision({ expectedGraphId, selectedGraphId: this.activeGraph.id });
    const { activeRevision, submittedRevision } = this.state.graphStatus;
    if (intent.revision <= submittedRevision)
      throw staleRevision({ activeRevision, submittedRevision, proposedRevision: intent.revision });
    const document = preserveMockProcessorState(this.state.graph, intent);
    this.state.graph = document;
    this.activeGraph.document = structuredClone(document);
    this.state.graphStatus = {
      activeRevision: document.revision,
      protocolVersion: 1,
      submittedRevision: document.revision,
    };
    return { protocolVersion: 1, revision: document.revision, state: 'active' };
  }

  public async getGraph(): Promise<RuntimeGraphDocument> {
    return this.state.graph;
  }

  public async getGraphStatus(): Promise<GraphRuntimeStatus> {
    return this.state.graphStatus;
  }

  public async getMeters(): Promise<MeterSnapshot> {
    return (
      this.state.meters ?? {
        nodes: [],
        protocolVersion: 1,
        revision: this.state.graphStatus.activeRevision,
      }
    );
  }

  public async getProcessors(): Promise<ExternalProcessorCatalog> {
    return this.state.processors ?? { processors: [], protocolVersion: 1 };
  }

  public async retryProcessor(request: ProcessorRetryRequest): Promise<GraphApplyResponse> {
    const failed = this.state.status.failedProcessors ?? [];
    const target = this.state.graph.nodes.find((node) => node.id === request.nodeId);
    if (
      request.graphId !== this.activeGraph.id ||
      request.expectedRevision !== this.state.graph.revision ||
      request.expectedCatalogRevision !== this.catalogRevision
    )
      throw staleRevision({
        selectedGraphId: this.activeGraph.id,
        submittedRevision: this.state.graph.revision,
        catalogRevision: this.catalogRevision,
      });
    if (!target) throw new Error('node_not_found');
    if (target.type !== 'provider' || target.control?.enabled === false)
      throw new Error('node_disabled');
    if (
      !failed.some(
        (entry) =>
          entry.graphId === request.graphId &&
          entry.nodeId === request.nodeId &&
          entry.instanceId === request.expectedInstanceId,
      )
    )
      throw new Error('processor_instance_stale');

    const revision = this.state.graph.revision + 1;
    this.state.graph = { ...this.state.graph, revision };
    this.activeGraph.document = structuredClone(this.state.graph);
    this.state.graphStatus = {
      activeRevision: revision,
      protocolVersion: 1,
      submittedRevision: revision,
    };
    this.state.status.failedProcessors = failed.filter(
      (entry) =>
        entry.graphId !== request.graphId ||
        entry.nodeId !== request.nodeId ||
        entry.instanceId !== request.expectedInstanceId,
    );
    return { protocolVersion: 1, revision, state: 'active' };
  }

  public async getProcessorEditor(nodeId: string): Promise<ProcessorEditorStatus> {
    return this.editorStates.get(nodeId) ?? closedEditor(nodeId);
  }

  public async getProcessorEditorFrame(_nodeId: string): Promise<ProcessorEditorFrame> {
    throw new Error('Native editor images are unavailable in Storybook');
  }

  public async sendProcessorEditorInput(
    _nodeId: string,
    _input: ProcessorEditorInput,
  ): Promise<void> {}

  public async rescanProcessors(_paths: string[]): Promise<ExternalProcessorCatalog> {
    return this.getProcessors();
  }

  public async getStatus(): Promise<MixRelayStatus> {
    return this.state.status;
  }

  public async setProcessorParameter(
    nodeId: string,
    parameterId: number,
    value: number,
  ): Promise<ProcessorParameterUpdateResponse> {
    const node = this.state.graph.nodes.find((candidate) => candidate.id === nodeId);
    if (
      node?.type !== 'provider' ||
      !Number.isInteger(parameterId) ||
      parameterId < 0 ||
      !Number.isFinite(value) ||
      value < 0 ||
      value > 1
    )
      throw new Error('graph_invalid');
    const config = record(node.config);
    const parameters = record(config.parameters);
    node.config = {
      ...config,
      parameters: {
        ...parameters,
        parameterValues: { ...record(parameters.parameterValues), [parameterId]: value },
      },
    };
    this.activeGraph.document = structuredClone(this.state.graph);
    return { nodeId, parameterId, protocolVersion: 1, value };
  }

  public async setProcessorEditor(nodeId: string, open: boolean): Promise<ProcessorEditorStatus> {
    const status = {
      ...closedEditor(nodeId),
      state: open ? ('open' as const) : ('closed' as const),
    };
    this.editorStates.set(nodeId, status);
    return status;
  }

  public async listCapturableApplications(): Promise<CapturableApplication[]> {
    return this.state.applications ?? [];
  }

  public async getVirtualDeviceAppActivity(): Promise<Record<string, DeviceAppActivity[]>> {
    return structuredClone(this.state.deviceAppActivity ?? {});
  }

  public async listAudioDevices(): Promise<AudioDeviceDescriptor[]> {
    return this.state.audioDevices ?? [];
  }
}

function record(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};
}

/** The mock has no vendor instance: its current document stands in for captured live state. */
function preserveMockProcessorState(current: RuntimeGraphDocument, request: GraphApplyRequest) {
  const { processorParameterChanges = [], ...document } = structuredClone(request);
  const previous = new Map(current.nodes.map((node) => [node.id, node]));
  const seen = new Set<string>();
  for (const change of processorParameterChanges) {
    const old = previous.get(change.nodeId);
    const next = document.nodes.find((node) => node.id === change.nodeId);
    const key = JSON.stringify([change.nodeId, change.parameterId]);
    if (
      seen.has(key) ||
      old?.type !== 'provider' ||
      next?.type !== 'provider' ||
      record(old.config).typeId !== change.typeId ||
      record(next.config).typeId !== change.typeId ||
      !Number.isInteger(change.parameterId) ||
      change.parameterId < 0 ||
      !Number.isFinite(change.value) ||
      change.value < 0 ||
      change.value > 1
    )
      throw new Error('graph_invalid');
    seen.add(key);
  }
  if (processorParameterChanges.length > 4096) throw new Error('graph_invalid');
  document.nodes = document.nodes.map((node) => {
    const old = previous.get(node.id);
    const config = record(node.config);
    const oldConfig = record(old?.config);
    if (node.type !== 'provider' || old?.type !== 'provider' || config.typeId !== oldConfig.typeId)
      return node;
    const parameters = record(oldConfig.parameters);
    const values = { ...record(parameters.parameterValues) };
    for (const change of processorParameterChanges)
      if (change.nodeId === node.id) values[String(change.parameterId)] = change.value;
    return {
      ...node,
      config: { ...config, parameters: { ...parameters, parameterValues: values } },
    };
  });
  return document;
}

function closedEditor(nodeId: string): ProcessorEditorStatus {
  return {
    failure: null,
    height: null,
    nodeId,
    protocolVersion: 1,
    resizable: false,
    state: 'closed',
    width: null,
  };
}
