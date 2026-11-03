import { Channel } from '@tauri-apps/api/core';
import {
  TelemetryHub,
  type TelemetryStreamListener,
} from '../../application/telemetry/TelemetryHub';
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
import type { MixRelayStatus, VirtualEndpointFormatRequest } from '../../domain/control/status';
import type { ControlPort } from '../../ports/control/ControlPort';
import type { MeterTelemetryListener, StateListener } from '../../ports/control/TelemetryPort';
import type { Scheduler } from '../../ports/Scheduler';
import { timerScheduler } from '../browser/timers';
import {
  type Decoder,
  nonNegativeInteger,
  nullable,
  object,
  string,
  unknownValue,
} from '../decode';
import {
  type NativeInvoke,
  type NativeListen,
  tauriInvoke,
  tauriListen,
} from '../tauri/nativeInvoke';
import {
  audioDevices,
  capturableApplications,
  deviceAppActivity,
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
  recorderSettings,
  recorderStatus,
  runtimeGraphDocument,
} from './protocolDecoders';
import { decodeReply, refusal } from './responses';
import { TelemetryDecoder } from './telemetryCodec';

/** The native side keeps its end; the webview only receives windows (decoded or packed) and
 * an end marker. */
type MessageChannel = { onmessage: (message: unknown) => void };

/** The terminal marker. Checked without `in`: WebKit rejects a minified loop header whose
 * nested arrow body uses it (see scripts/check-webkit-syntax.mjs). */
function isEnd(message: unknown): boolean {
  return (
    typeof message === 'object' &&
    message !== null &&
    !(message instanceof ArrayBuffer) &&
    Object.hasOwn(message, 'ended')
  );
}

/** The desktop forwards engine replies with their HTTP status. */
const proxyResponse = object<{ status: number; body: unknown }>({
  status: nonNegativeInteger,
  body: unknownValue,
});

export class TauriControlPort implements ControlPort {
  private readonly telemetry: TelemetryHub;
  public subscribeState(listener: StateListener) {
    return this.telemetry.subscribeState(listener);
  }
  public subscribeMeters(listener: MeterTelemetryListener) {
    return this.telemetry.subscribeMeters(listener);
  }
  public getNodeControls(graphId: string): Promise<GraphNodeControls> {
    return this.call(graphNodeControls, 'control_get_node_controls', { graphId });
  }
  public updateNodeControl(
    graphId: string,
    nodeId: string,
    request: NodeControlRequest,
  ): Promise<NodeControlResponse> {
    return this.call(nodeControlResponse, 'control_update_node_control', {
      graphId,
      nodeId,
      request,
    });
  }
  public setEnginePower(enabled: boolean): Promise<GraphApplyResponse> {
    return this.call(graphApplyResponse, 'control_set_engine_power', { enabled });
  }
  public onWorkspaceFile(
    listener: (action: 'new' | 'open' | 'save' | 'save_as') => void,
  ): Promise<() => void> {
    return this.listen('workspace-file', (payload) => {
      if (payload === 'new' || payload === 'open' || payload === 'save' || payload === 'save_as')
        listener(payload);
    });
  }

  public async workspaceFile(
    request: Parameters<NonNullable<ControlPort['workspaceFile']>>[0],
  ): Promise<GraphCatalog | null> {
    const response = decodeReply(
      nullable(proxyResponse),
      await this.invokeCommand('workspace_file', { request }),
      'workspace_file',
    );
    if (!response) return null;
    if (response.status < 200 || response.status >= 300)
      throw refusal(response.status, response.body);
    return decodeReply(graphCatalog, response.body, 'workspace_file');
  }
  public async recordingSettings(graphId: string, nodeId: string): Promise<RecorderSettings> {
    return this.direct(recorderSettings, 'recording_settings', { graphId, nodeId });
  }

  public async setRecordingSettings(
    graphId: string,
    nodeId: string,
    settings: RecorderSettings,
    expectedRevision: number,
  ): Promise<RecorderSettings> {
    return this.direct(recorderSettings, 'save_recording_settings', {
      graphId,
      nodeId,
      settings,
      expectedRevision,
    });
  }

  public recorderStatus(graphId: string, nodeId: string): Promise<RecorderStatus> {
    return this.call(recorderStatus, 'control_get_recorder_status', { graphId, nodeId });
  }

  public controlRecorder(
    graphId: string,
    nodeId: string,
    request: RecorderRequest,
    expectedRevision: number,
  ): Promise<RecorderStatus> {
    return this.call(recorderStatus, 'control_set_recorder', {
      graphId,
      nodeId,
      request: { expectedRevision, request },
    });
  }

  public async selectRecordingFolder(options: {
    defaultPath?: string;
    title?: string;
  }): Promise<string | null> {
    return this.direct(nullable(string), 'select_recording_folder', options);
  }

  public setVirtualEndpointSampleRate(
    request: VirtualEndpointFormatRequest,
  ): Promise<GraphApplyResponse> {
    return this.call(graphApplyResponse, 'control_set_virtual_endpoint_sample_rate', { request });
  }
  public getGraphLayout(graphId: string): Promise<GraphLayoutResponse> {
    return this.call(graphLayoutResponse, 'control_get_graph_layout', { graphId });
  }

  public setGraphLayout(
    graphId: string,
    request: GraphLayoutRequest,
  ): Promise<GraphLayoutResponse> {
    return this.call(graphLayoutResponse, 'control_set_graph_layout', { graphId, request });
  }

  public getGraphCatalog(): Promise<GraphCatalog> {
    return this.call(graphCatalog, 'control_get_graph_catalog');
  }

  public mutateGraphCatalog(request: GraphCatalogRequest): Promise<GraphCatalog> {
    return this.call(graphCatalog, 'control_mutate_graph_catalog', { request });
  }
  public reconfigureSession(request: {
    expectedRevision: number;
    format: { sampleRate: number; blockFrames: number };
  }): Promise<GraphApplyResponse> {
    return this.call(graphApplyResponse, 'control_reconfigure_session', { request });
  }
  public constructor(
    private readonly invokeCommand: NativeInvoke = tauriInvoke,
    private readonly createChannel: () => MessageChannel = () => new Channel<unknown>(),
    scheduler: Scheduler = timerScheduler,
    private readonly listen: NativeListen = tauriListen,
  ) {
    this.telemetry = new TelemetryHub((listener) => this.openTelemetry(listener), scheduler);
  }

  public applyGraph(document: GraphApplyRequest): Promise<GraphApplyResponse> {
    return this.call(graphApplyResponse, 'control_apply_graph', { document });
  }

  public getGraph(): Promise<RuntimeGraphDocument> {
    return this.call(runtimeGraphDocument, 'control_get_graph');
  }

  public async isOperationPending(operationId: string): Promise<boolean> {
    const response = decodeReply(
      proxyResponse,
      await this.invokeCommand('control_get_operation', { operationId }),
      'control_get_operation',
    );
    if (response.status !== 202) return false;
    return (
      typeof response.body === 'object' &&
      response.body !== null &&
      'operationId' in response.body &&
      response.body.operationId === operationId
    );
  }

  public getGraphStatus(): Promise<GraphRuntimeStatus> {
    return this.call(graphRuntimeStatus, 'control_get_graph_status');
  }

  public getMeters(): Promise<MeterSnapshot> {
    return this.call(meterSnapshot, 'control_get_meters');
  }

  /** Each consumed window returns one credit, so a stalled webview receives nothing more
   * instead of accumulating messages natively. Windows can arrive before the id does. */
  private async openTelemetry(listener: TelemetryStreamListener): Promise<() => void> {
    const channel = this.createChannel();
    let id: number | undefined;
    let owed = 0;
    let active = true;
    const decoder = new TelemetryDecoder();
    const credit = () => {
      if (id === undefined) owed += 1;
      else void this.invokeCommand('control_telemetry_credit', { id }).catch(() => undefined);
    };
    channel.onmessage = (message) => {
      // After unsubscribing, nothing is delivered and no credit returns, so even a lost
      // unsubscribe ends natively once credit runs out.
      if (!active) return;
      if (isEnd(message)) {
        active = false;
        return listener.ended();
      }
      try {
        listener.window(
          message instanceof ArrayBuffer ? decoder.raw(message) : decoder.window(message),
        );
      } catch (error) {
        // The stream cannot continue past a window it cannot read; say why, once.
        console.error('Telemetry window rejected; the meter stream ends', error);
        active = false;
        listener.ended();
      }
      credit();
    };
    id = decodeReply(
      nonNegativeInteger,
      await this.invokeCommand('control_subscribe_telemetry', { channel }),
      'control_subscribe_telemetry',
    );
    for (; owed > 0; owed -= 1) credit();
    const page = typeof document === 'undefined' ? undefined : document;
    const visibility = () => {
      if (active)
        void this.invokeCommand('control_telemetry_visibility', {
          id,
          visible: page?.visibilityState !== 'hidden',
        }).catch(() => undefined);
    };
    page?.addEventListener('visibilitychange', visibility);
    if (page?.visibilityState === 'hidden') visibility();
    return () => {
      active = false;
      page?.removeEventListener('visibilitychange', visibility);
      void this.invokeCommand('control_unsubscribe_telemetry', { id }).catch(() => undefined);
    };
  }

  public getProcessors(): Promise<ExternalProcessorCatalog> {
    return this.call(externalProcessorCatalog, 'control_get_processors');
  }

  public retryProcessor(request: ProcessorRetryRequest): Promise<GraphApplyResponse> {
    return this.call(graphApplyResponse, 'control_retry_processor', { request });
  }

  public getProcessorEditor(nodeId: string): Promise<ProcessorEditorStatus> {
    return this.call(processorEditorStatus, 'control_get_processor_editor', { nodeId });
  }

  public getProcessorEditorFrame(nodeId: string): Promise<ProcessorEditorFrame> {
    return this.call(processorEditorFrame, 'control_get_processor_editor_frame', { nodeId });
  }

  public async sendProcessorEditorInput(
    nodeId: string,
    input: ProcessorEditorInput,
  ): Promise<void> {
    await this.call(unknownValue, 'control_send_processor_editor_input', { nodeId, input });
  }

  public rescanProcessors(paths: string[]): Promise<ExternalProcessorCatalog> {
    return this.call(externalProcessorCatalog, 'control_rescan_processors', { paths });
  }

  public async selectProcessorDirectory(): Promise<string | null> {
    return this.direct(nullable(string), 'select_processor_directory');
  }

  public getStatus(): Promise<MixRelayStatus> {
    return this.call(mixRelayStatus, 'control_get_status');
  }

  public setProcessorParameter(
    nodeId: string,
    parameterId: number,
    value: number,
    expectedRevision: number,
  ): Promise<ProcessorParameterUpdateResponse> {
    return this.call(processorParameterUpdateResponse, 'control_set_processor_parameter', {
      nodeId,
      parameterId,
      value,
      expectedRevision,
    });
  }

  public setProcessorEditor(
    nodeId: string,
    open: boolean,
    expectedRevision: number,
  ): Promise<ProcessorEditorStatus> {
    return this.call(processorEditorStatus, 'control_set_processor_editor', {
      nodeId,
      open,
      expectedRevision,
    });
  }

  public async listCapturableApplications(): Promise<CapturableApplication[]> {
    return this.direct(capturableApplications, 'list_capturable_applications');
  }

  /** An IPC rejection stays unwrapped: it means activity is unavailable, not a failed edit. */
  public async getVirtualDeviceAppActivity(): Promise<Record<string, DeviceAppActivity[]>> {
    return decodeReply(
      deviceAppActivity,
      await this.invokeCommand('list_device_app_activity'),
      'list_device_app_activity',
    );
  }

  public async listAudioDevices(): Promise<AudioDeviceDescriptor[]> {
    return this.direct(audioDevices, 'list_audio_devices');
  }

  /** A desktop command answered directly, without the engine's status envelope. */
  private async direct<T>(
    decoder: Decoder<T>,
    command: string,
    ...args: [Record<string, unknown>?]
  ): Promise<T> {
    let reply: unknown;
    try {
      reply = await this.invokeCommand(command, ...args);
    } catch (error) {
      throw invocationError(command, error);
    }
    return decodeReply(decoder, reply, command);
  }

  /** An engine command the desktop proxies as `{ status, body }`. */
  private async call<T>(
    decoder: Decoder<T>,
    command: string,
    args?: Record<string, unknown>,
  ): Promise<T> {
    const response = await this.direct(proxyResponse, command, args);
    if (response.status < 200 || response.status >= 300)
      throw refusal(response.status, response.body);
    return decodeReply(decoder, response.body, command);
  }
}

function invocationError(command: string, cause: unknown): ControlPortError {
  // IPC rejection can mean an unknown command or invalid payload while the engine is healthy.
  // Only the native HTTP boundary can identify an actual engine connection failure.
  const reason = cause instanceof Error ? cause.message : String(cause);
  console.error(`Tauri command ${command} failed`, cause);
  return new ControlPortError(500, {
    code: 'control_request_failed',
    parameters: { command, reason },
  });
}
