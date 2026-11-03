import type {
  AudioDeviceDescriptor,
  CapturableApplication,
  DeviceAppActivity,
} from '../../domain/control/inventory';
import type { ExternalProcessorCatalog } from '../../domain/control/processors';

/** What can be added to a graph: plug-ins, applications and audio devices. */
export interface InventoryPort {
  getProcessors(): Promise<ExternalProcessorCatalog>;
  rescanProcessors(paths: string[]): Promise<ExternalProcessorCatalog>;
  selectProcessorDirectory?(): Promise<string | null>;
  listCapturableApplications?(): Promise<CapturableApplication[]>;
  listAudioDevices?(): Promise<AudioDeviceDescriptor[]>;
  /** A successful missing UID is observed empty; rejection means unavailable. */
  getVirtualDeviceAppActivity?(): Promise<Record<string, DeviceAppActivity[]>>;
}
