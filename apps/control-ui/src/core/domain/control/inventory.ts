export type RuntimeApplicationSelector =
  | { bundle_id: string; include_descendants: boolean; match: 'bundle_id' }
  | { match: 'executable_path'; path: string }
  | { include_descendants: boolean; match: 'process_id'; process_id: number }
  | { bundle_ids: string[]; match: 'all_except' };

export interface CapturableApplication {
  bundleId: string;
  displayName: string;
  iconDataUrl: string | null;
  processId: number;
  selector: RuntimeApplicationSelector;
}

export interface AudioDeviceDescriptor {
  displayName: string;
  inputChannels: number;
  isDefaultInput: boolean;
  isDefaultOutput: boolean;
  outputChannels: number;
  uid: string;
}

/** Observed native device clients; flags describe OS playback/capture, not graph state. */
export interface DeviceAppActivity {
  bundleId: string;
  displayName: string;
  iconDataUrl?: string;
  processId: number;
  playing: boolean;
  recording: boolean;
}
