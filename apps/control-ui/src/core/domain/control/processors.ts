export interface ProcessorParameterDescriptor {
  canAutomate: boolean;
  defaultValue: number;
  id: number;
  isReadOnly: boolean;
  name: string;
  stepCount: number;
}

export type ProviderBusKind = 'main' | 'sidechain' | 'auxiliary';

export type ProviderChannelFormat = 'mono' | 'stereo' | 'surround5_1' | 'surround7_1' | 'discrete';

export interface ProviderBus {
  channelFormat: ProviderChannelFormat;
  channels: number;
  id: string;
  kind: ProviderBusKind;
  name: string;
}

export interface ProviderPorts {
  inputs: ProviderBus[];
  layoutId: string;
  outputs: ProviderBus[];
}

export interface ProviderNodeConfig {
  parameters?: Record<string, unknown>;
  ports?: ProviderPorts;
  /** The plug-in identity; layoutId is stored only inside ports. */
  typeId: string;
}

export interface ExternalProcessorDescriptor {
  displayName: string;
  format: 'au' | 'auv3' | 'clap' | 'lv2' | 'vst3';
  hasEditor: boolean;
  layouts: ProviderPorts[];
  defaultLayoutId: string;
  parameters: ProcessorParameterDescriptor[];
  typeId: string;
  vendor: string;
  version: string;
}

export interface ExternalProcessorDiagnostic {
  format: ExternalProcessorDescriptor['format'];
  stage: 'scan' | 'probe';
  pluginId?: string;
  pluginName?: string;
  message: string;
}

export interface ExternalProcessorCatalog {
  processors: ExternalProcessorDescriptor[];
  protocolVersion: 1;
  /** A partial native scan is not a successful empty catalog. */
  diagnostics?: ExternalProcessorDiagnostic[];
}

export interface ProcessorParameterUpdateResponse {
  nodeId: string;
  parameterId: number;
  protocolVersion: 1;
  value: number;
  /** Audio accepted the value, but durable graph storage may have failed. */
  persistenceError?: string;
}

export type ProcessorEditorState = 'unavailable' | 'closed' | 'opening' | 'open' | 'failed';

export type ProcessorEditorFailure =
  | 'no_editor'
  | 'display_unavailable'
  | 'host_window_failed'
  | 'plugin_rejected'
  | 'worker_unavailable';

export interface ProcessorEditorStatus {
  failure: ProcessorEditorFailure | null;
  height: number | null;
  nodeId: string;
  protocolVersion: 1;
  resizable: boolean;
  state: ProcessorEditorState;
  width: number | null;
}

export interface ProcessorEditorFrame {
  instanceId: string;
  session: number;
  width: number;
  height: number;
  pngBase64: string;
}

export interface ProcessorEditorInput {
  instanceId: string;
  session: number;
  kind: 'down' | 'move' | 'up' | 'cancel';
  x: number;
  y: number;
  modifiers: number;
}
