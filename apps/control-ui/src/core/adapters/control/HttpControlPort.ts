import { TelemetryHub } from '../../application/telemetry/TelemetryHub';
import type {
  GraphCatalog,
  GraphCatalogRequest,
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
} from '../../domain/control/graph';
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
import type { MixRelayStatus, VirtualEndpointFormatRequest } from '../../domain/control/status';
import type { ControlPort } from '../../ports/control/ControlPort';
import type { MeterTelemetryListener, StateListener } from '../../ports/control/TelemetryPort';
import type { Scheduler } from '../../ports/Scheduler';
import { timerScheduler } from '../browser/timers';
import { type Decoder, unknownValue } from '../decode';
import { openHttpTelemetry } from './httpTelemetry';
import {
  externalProcessorCatalog,
  graphApplyResponse,
  graphCatalog,
  graphLayoutResponse,
  graphNodeControls,
  graphRuntimeStatus,
  meterSnapshot,
  mixRelayStatus,
  nodeControlResponse,
  processorEditorFrame,
  processorEditorStatus,
  processorParameterUpdateResponse,
  recorderStatus,
  runtimeGraphDocument,
} from './protocolDecoders';
import { decodeReply, refusal } from './responses';

// The engine listens only on a user-owned Unix socket. In the browser development surface,
// `vite dev` forwards this same-origin path to it; Tauri uses native IPC instead.
const LOCAL_CONTROL_ORIGIN = '/engine';
const OPERATION_HEADER = 'X-MixRelay-Operation';
const OPERATION_RESULT_HEADER = 'X-MixRelay-Operation-Result';
const OPERATION_POLL_INTERVAL_MS = 100;
const OPERATION_POLL_TIMEOUT_MS = 5000;
const OPERATION_ACK_TIMEOUT_MS = 1000;
// Reads are safe to abandon; mutations are never cut short, so a sent edit is not reported
// as failed while the engine may still apply it.
const READ_TIMEOUT_MS = 5000;
const MAX_OPERATION_ID_LENGTH = 128;

export class HttpControlPort implements ControlPort {
  private readonly telemetry: TelemetryHub;
  public subscribeState(listener: StateListener) {
    return this.telemetry.subscribeState(listener);
  }
  public subscribeMeters(listener: MeterTelemetryListener) {
    return this.telemetry.subscribeMeters(listener);
  }
  public async recordingSettings(_graphId: string, _nodeId: string): Promise<RecorderSettings> {
    throw new ControlPortError(501, {
      code: 'control_request_failed',
      parameters: { reason: 'Recording settings require the desktop application.' },
    });
  }

  public async setRecordingSettings(
    graphId: string,
    nodeId: string,
    _settings: RecorderSettings,
    _expectedRevision: number,
  ): Promise<RecorderSettings> {
    return this.recordingSettings(graphId, nodeId);
  }
  public getNodeControls(graphId: string): Promise<GraphNodeControls> {
    return this.json(
      graphNodeControls,
      `/api/v1/graphs/${encodeURIComponent(graphId)}/nodes/controls`,
    );
  }
  public updateNodeControl(
    graphId: string,
    nodeId: string,
    request: NodeControlRequest,
  ): Promise<NodeControlResponse> {
    return this.json(
      nodeControlResponse,
      `/api/v1/graphs/${encodeURIComponent(graphId)}/nodes/${encodeURIComponent(nodeId)}/control`,
      {
        method: 'PUT',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify(request),
      },
    );
  }
  public recorderStatus(graphId: string, nodeId: string): Promise<RecorderStatus> {
    return this.json(
      recorderStatus,
      `/api/v1/graphs/${encodeURIComponent(graphId)}/recorders/${encodeURIComponent(nodeId)}`,
    );
  }

  public controlRecorder(
    graphId: string,
    nodeId: string,
    request: RecorderRequest,
    expectedRevision: number,
  ): Promise<RecorderStatus> {
    return this.json(
      recorderStatus,
      `/api/v1/graphs/${encodeURIComponent(graphId)}/recorders/${encodeURIComponent(nodeId)}`,
      {
        method: 'PUT',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ expectedRevision, request }),
      },
    );
  }

  public setVirtualEndpointSampleRate(
    request: VirtualEndpointFormatRequest,
  ): Promise<GraphApplyResponse> {
    return this.json(graphApplyResponse, '/api/v1/audio/virtual-endpoint/format', {
      method: 'PUT',
      body: JSON.stringify(request),
      headers: { 'content-type': 'application/json' },
    });
  }
  public getGraphLayout(graphId: string): Promise<GraphLayoutResponse> {
    return this.json(graphLayoutResponse, `/api/v1/graphs/${encodeURIComponent(graphId)}/layout`);
  }

  public setGraphLayout(
    graphId: string,
    request: GraphLayoutRequest,
  ): Promise<GraphLayoutResponse> {
    return this.json(graphLayoutResponse, `/api/v1/graphs/${encodeURIComponent(graphId)}/layout`, {
      body: JSON.stringify(request),
      headers: { 'content-type': 'application/json' },
      method: 'PUT',
    });
  }

  public getGraphCatalog(): Promise<GraphCatalog> {
    return this.json(graphCatalog, '/api/v1/graphs');
  }

  public mutateGraphCatalog(request: GraphCatalogRequest): Promise<GraphCatalog> {
    return this.json(graphCatalog, '/api/v1/graphs', {
      body: JSON.stringify(request),
      headers: { 'content-type': 'application/json' },
      method: 'PUT',
    });
  }
  public reconfigureSession(request: {
    expectedRevision: number;
    format: { sampleRate: number; blockFrames: number };
  }): Promise<GraphApplyResponse> {
    return this.json(graphApplyResponse, '/api/v1/audio/session', {
      body: JSON.stringify(request),
      headers: { 'content-type': 'application/json' },
      method: 'PUT',
    });
  }
  public constructor(
    private readonly origin = LOCAL_CONTROL_ORIGIN,
    private readonly request: typeof fetch = fetch,
    private readonly scheduler: Scheduler = timerScheduler,
  ) {
    this.telemetry = new TelemetryHub(
      (listener) => openHttpTelemetry(this.origin, this.request, listener, scheduler),
      scheduler,
    );
  }

  public applyGraph(document: GraphApplyRequest): Promise<GraphApplyResponse> {
    return this.json(graphApplyResponse, '/api/v1/graph', {
      body: JSON.stringify(document),
      headers: { 'content-type': 'application/json' },
      method: 'PUT',
    });
  }

  public getGraph(): Promise<RuntimeGraphDocument> {
    return this.json(runtimeGraphDocument, '/api/v1/graph');
  }

  public async isOperationPending(operationId: string): Promise<boolean> {
    const request = this.request;
    const response = await request(
      `${this.origin}/api/v1/operations/${encodeURIComponent(operationId)}`,
      {
        signal: AbortSignal.timeout(READ_TIMEOUT_MS),
      },
    );
    if (response.status !== 202) return false;
    const body: unknown = await response.json();
    validateOperationReceipt(body, operationId);
    return pendingOperationId(response) === operationId;
  }

  public getGraphStatus(): Promise<GraphRuntimeStatus> {
    return this.json(graphRuntimeStatus, '/api/v1/graph/status');
  }

  public getMeters(): Promise<MeterSnapshot> {
    return this.json(meterSnapshot, '/api/v1/meters');
  }

  public getProcessors(): Promise<ExternalProcessorCatalog> {
    return this.json(externalProcessorCatalog, '/api/v1/processors');
  }

  public retryProcessor(request: ProcessorRetryRequest): Promise<GraphApplyResponse> {
    return this.json(graphApplyResponse, '/api/v1/processors/retry', {
      body: JSON.stringify(request),
      headers: { 'content-type': 'application/json' },
      method: 'PUT',
    });
  }

  public getProcessorEditor(nodeId: string): Promise<ProcessorEditorStatus> {
    return this.json(
      processorEditorStatus,
      `/api/v1/processors/${encodeURIComponent(nodeId)}/editor`,
    );
  }

  public getProcessorEditorFrame(nodeId: string): Promise<ProcessorEditorFrame> {
    return this.json(
      processorEditorFrame,
      `/api/v1/processors/${encodeURIComponent(nodeId)}/editor/frame`,
    );
  }

  public async sendProcessorEditorInput(
    nodeId: string,
    input: ProcessorEditorInput,
  ): Promise<void> {
    await this.json(unknownValue, `/api/v1/processors/${encodeURIComponent(nodeId)}/editor/input`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(input),
    });
  }

  public rescanProcessors(paths: string[]): Promise<ExternalProcessorCatalog> {
    return this.json(externalProcessorCatalog, '/api/v1/processors/scan', {
      body: JSON.stringify({ paths }),
      headers: { 'content-type': 'application/json' },
      method: 'PUT',
    });
  }

  public getStatus(): Promise<MixRelayStatus> {
    return this.json(mixRelayStatus, '/api/v1/status');
  }

  public setProcessorParameter(
    nodeId: string,
    parameterId: number,
    value: number,
    expectedRevision: number,
  ): Promise<ProcessorParameterUpdateResponse> {
    return this.json(
      processorParameterUpdateResponse,
      `/api/v1/processors/${encodeURIComponent(nodeId)}/parameters/${parameterId}`,
      {
        body: JSON.stringify({ value, expectedRevision }),
        headers: { 'content-type': 'application/json' },
        method: 'PUT',
      },
    );
  }

  public setProcessorEditor(
    nodeId: string,
    open: boolean,
    expectedRevision: number,
  ): Promise<ProcessorEditorStatus> {
    return this.json(
      processorEditorStatus,
      `/api/v1/processors/${encodeURIComponent(nodeId)}/editor`,
      {
        body: JSON.stringify({ open, expectedRevision }),
        headers: { 'content-type': 'application/json' },
        method: 'PUT',
      },
    );
  }

  private async json<T>(decoder: Decoder<T>, path: string, init?: RequestInit): Promise<T> {
    // Browser fetch is a Web IDL method, not an adapter method. Calling it with
    // this port as receiver throws before any HTTP request is sent in WebKit.
    const request = this.request;
    if (typeof init?.body === 'string') {
      const body = JSON.parse(init.body);
      if (typeof body.clientOperationId === 'string') {
        const headers = new Headers(init.headers);
        headers.set('x-mixrelay-operation-request', body.clientOperationId);
        delete body.clientOperationId;
        init = { ...init, headers, body: JSON.stringify(body) };
      }
    }
    const response = await request(
      `${this.origin}${path}`,
      init ?? { signal: AbortSignal.timeout(READ_TIMEOUT_MS) },
    );
    const body: unknown = await response.json();
    const operationId = pendingOperationId(response);
    if (operationId !== undefined) {
      validateOperationReceipt(body, operationId);
      return this.pollOperation(decoder, path, operationId);
    }
    if (!response.ok) throw refusal(response.status, body);
    return decodeReply(decoder, body, path);
  }

  private async pollOperation<T>(
    decoder: Decoder<T>,
    requestPath: string,
    operationId: string,
  ): Promise<T> {
    const path = `/api/v1/operations/${encodeURIComponent(operationId)}`;
    const request = this.request;
    for (;;) {
      await new Promise<void>((resolve) =>
        this.scheduler.after(OPERATION_POLL_INTERVAL_MS, resolve),
      );
      const response = await request(`${this.origin}${path}`, {
        method: 'GET',
        signal: AbortSignal.timeout(OPERATION_POLL_TIMEOUT_MS),
      });
      const body: unknown = await response.json();
      const pendingId = pendingOperationId(response);
      if (pendingId !== undefined) {
        if (pendingId !== operationId) {
          throw new Error('local control operation identity changed or was invalid');
        }
        validateOperationReceipt(body, operationId);
        continue;
      }
      validateTerminalOperation(response, operationId);
      await this.acknowledgeOperation(path);
      if (!response.ok) throw refusal(response.status, body);
      return decodeReply(decoder, body, requestPath);
    }
  }

  private async acknowledgeOperation(path: string): Promise<void> {
    try {
      const request = this.request;
      await request(`${this.origin}${path}`, {
        method: 'DELETE',
        signal: AbortSignal.timeout(OPERATION_ACK_TIMEOUT_MS),
      });
    } catch {
      // The terminal response is already authoritative; acknowledgement only reclaims
      // the server-side receipt and must not replace the observed operation result.
    }
  }
}

function pendingOperationId(response: Response): string | undefined {
  const operationId = response.headers.get(OPERATION_HEADER);
  if (operationId === null) return undefined;
  if (
    response.status !== 202 ||
    operationId.length === 0 ||
    operationId.length > MAX_OPERATION_ID_LENGTH
  ) {
    throw new Error('invalid operation receipt');
  }
  return operationId;
}

function validateOperationReceipt(body: unknown, operationId: string): void {
  if (
    typeof body !== 'object' ||
    body === null ||
    !('operationId' in body) ||
    typeof body.operationId !== 'string' ||
    body.operationId !== operationId
  ) {
    throw new Error('local control operation identity changed or was invalid');
  }
}

function validateTerminalOperation(response: Response, operationId: string): void {
  if (response.headers.get(OPERATION_RESULT_HEADER) !== operationId) {
    throw new Error('local control operation result could not be confirmed');
  }
}
