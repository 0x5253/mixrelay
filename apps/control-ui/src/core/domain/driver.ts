import type { NativeSampleFormat, NativeStreamFormat } from './control/nativeFormat';
import type { VirtualEndpointSettings, VirtualEndpointSpec } from './control/status';

export type NativeDirection = 'render' | 'capture';

export interface DriverSnapshot {
  protocolVersion: 1;
  revision: number;
  state: 'ready' | 'not_installed' | 'installed_not_loaded' | 'unavailable';
  installedVersion: string | null;
  settings: VirtualEndpointSettings;
  persistenceError?: string;
}

export type DriverAction =
  | { action: 'create'; spec: VirtualEndpointSpec }
  | { action: 'configure'; spec: VirtualEndpointSpec }
  | { action: 'remove'; endpointId: string }
  | { action: 'set_sample_format'; endpointId: string; sampleFormat: NativeSampleFormat }
  | {
      action: 'set_format';
      endpointId: string;
      direction: NativeDirection;
      format: NativeStreamFormat;
    };

export type DriverCommand = DriverAction & { expectedRevision: number };
