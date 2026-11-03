import type { NativeDeviceFormats } from './nativeFormat';

export type DriverState =
  | 'not_installed'
  | 'installing'
  | 'restart_required'
  | 'installed_not_loaded'
  | 'blocked'
  | 'unavailable'
  | 'invalid_permissions'
  | 'version_mismatch'
  | 'ready';

export interface MixRelayStatus {
  /** `unsaved`: the service could not yet save this state for its next start and keeps retrying.
   * `pauseReason`: why the engine paused itself (absent for a pause the user chose);
   * `repeatedFailures` means it ended unexpectedly several times in a row. */
  enginePower?: {
    enabled: boolean;
    transitioning: boolean;
    unsaved?: boolean;
    pauseReason?: 'repeatedFailures';
  };
  failedProcessors?: FailedProcessorInstance[];
  processorTimings?: ProcessorTiming[];
  processorLatencies?: ProcessorLatency[];
  recorderError?: string | null;
  protocolVersion: 1;
  driverState: DriverState;
  sampleRate: number | null;
  bufferFrames: number | null;
  underruns: number;
  overruns: number;
  audioSession?: AudioSessionStatus;
}

/** Opaque native instance identity for a processor whose DSP instance has failed. */
export interface FailedProcessorInstance {
  graphId: string;
  instanceId: string;
  nodeId: string;
}

export interface ProcessorTiming {
  graphId: string;
  nodeId: string;
  roundTripNs: number;
}

/** Cached frame-domain latency, not a wall-clock measurement or PDC guarantee. */
export interface ProcessorLatency {
  graphId: string;
  nodeId: string;
  transportFrames: number;
  /** null means unknown; zero is a reported zero-latency value. */
  intrinsicFrames: number | null;
}

export interface AudioSessionStatus {
  endpointPreferencesUnavailable?: boolean;
  virtualEndpointSettings?: VirtualEndpointSettings;
  processingState?: 'idle' | 'running' | 'stopped';
  discontinuities?: number;
  format: { sampleRate: number; blockFrames: number };
  preferredFormat?: { sampleRate: number; blockFrames: number } | null;
  clockAuthority: 'engine_timer' | 'device' | 'audio_server' | 'host';
  observedDeviceSampleRate: number | null;
  observedDeviceBufferFrames?: number | null;
  issue:
    | 'processing_failed'
    | 'application_unavailable'
    | 'application_ambiguous'
    | 'device_unavailable'
    | 'sample_rate_mismatch'
    | 'block_size_mismatch'
    | 'clock_not_synchronized'
    | 'invalid_format'
    | 'reconfiguration_failed'
    | 'preferences_unavailable'
    | 'stored_format_unavailable'
    | null;
  transportLatencyFrames: number | null;
  capabilities: {
    backend: 'core_audio' | 'pipe_wire' | 'contract_only';
    supportedSampleRates: number[];
    supportedBlockFrames: number[];
    canReconfigureLive: boolean;
    hardwareInput: boolean;
    hardwareOutput: boolean;
    virtualEndpoint: boolean;
    applicationCapture: boolean;
    sampleRateConversion: boolean;
    driftCorrection: boolean;
  };
}

export interface VirtualEndpointSettings {
  canCreate: boolean;
  canRename: boolean;
  renderChannels: number[];
  captureChannels: number[];
  creationTemplate: Omit<VirtualEndpointSpec, 'id' | 'displayName'> | null;
  devices: VirtualEndpointDevice[];
}

export interface VirtualEndpointDevice {
  nativeDeviceIds: string[];
  nativeFormats: NativeDeviceFormats;
  spec: VirtualEndpointSpec;
  observedSampleRate: number | null;
  supportedSampleRates: number[];
  canSetSampleRate: boolean;
}

export interface RuntimeStreamSpec {
  channelLayout: 'discrete' | 'mono' | 'stereo';
  channels: number;
}

export interface VirtualEndpointSpec {
  id: string;
  displayName: string;
  render?: RuntimeStreamSpec | null;
  capture?: RuntimeStreamSpec | null;
  supportedSampleRates: number[];
  defaultSampleRate: number;
}

export interface VirtualEndpointFormatRequest {
  endpointId: string;
  expectedRevision: number;
  sampleRate: number;
}
