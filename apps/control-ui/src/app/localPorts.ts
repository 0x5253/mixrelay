import { HttpControlPort } from '../core/adapters/control/HttpControlPort';
import { TauriControlPort } from '../core/adapters/control/TauriControlPort';
import { TauriDiagnosticsPort } from '../core/adapters/tauri/TauriDiagnosticsPort';
import { TauriDriverPort } from '../core/adapters/tauri/TauriDriverPort';
import { TauriMidiPort } from '../core/adapters/tauri/TauriMidiPort';
import { TauriNativeShell } from '../core/adapters/tauri/TauriNativeShell';
import { TauriRuntimePort } from '../core/adapters/tauri/TauriRuntimePort';
import { TauriUpdatesPort } from '../core/adapters/tauri/TauriUpdatesPort';
import type { ControlPort } from '../core/ports/control/ControlPort';
import type { DiagnosticsPort } from '../core/ports/DiagnosticsPort';
import type { DriverPort } from '../core/ports/DriverPort';
import type { MidiPort } from '../core/ports/MidiPort';
import type { NativeShellPort } from '../core/ports/NativeShellPort';
import type { RuntimePort } from '../core/ports/RuntimePort';
import type { UpdatesPort } from '../core/ports/UpdatesPort';

/** Every port the editor uses, chosen once for the environment it runs in. */
export interface LocalPorts {
  controlPort: ControlPort;
  updatesPort: UpdatesPort | null;
  driverPort: DriverPort | null;
  runtimePort: RuntimePort | null;
  diagnosticsPort: DiagnosticsPort | null;
  midiPort: MidiPort | null;
  nativeShell: NativeShellPort | null;
}

/** The packaged desktop app exposes Tauri's IPC bridge on the window. */
export function isNativeDesktop(page: object): boolean {
  return '__TAURI_INTERNALS__' in page;
}

/** The desktop owns every native capability. A browser reaches only the engine, through
 * the dev server's same-origin bridge, and never creates or asserts native state such as
 * update checks or Driver devices. */
export function createLocalPorts(native: boolean): LocalPorts {
  if (!native)
    return {
      controlPort: new HttpControlPort(),
      updatesPort: null,
      driverPort: null,
      runtimePort: null,
      diagnosticsPort: null,
      midiPort: null,
      nativeShell: null,
    };
  return {
    controlPort: new TauriControlPort(),
    updatesPort: new TauriUpdatesPort(),
    driverPort: new TauriDriverPort(),
    runtimePort: new TauriRuntimePort(),
    diagnosticsPort: new TauriDiagnosticsPort(),
    midiPort: new TauriMidiPort(),
    nativeShell: new TauriNativeShell(),
  };
}

let environmentPorts: LocalPorts | undefined;

/** The ports of the environment this page runs in, created on first use and then shared. */
export function localPorts(): LocalPorts {
  environmentPorts ??= createLocalPorts(isNativeDesktop(window));
  return environmentPorts;
}
