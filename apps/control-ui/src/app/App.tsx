import type { ControlPort } from '../core/ports/control/ControlPort';
import type { DiagnosticsPort } from '../core/ports/DiagnosticsPort';
import type { DriverPort } from '../core/ports/DriverPort';
import type { MidiPort } from '../core/ports/MidiPort';
import type { RuntimePort } from '../core/ports/RuntimePort';
import type { UpdatesPort } from '../core/ports/UpdatesPort';
import { AboutAccess } from '../features/app-info/AboutAccess';
import { DriverSessionAccess } from '../features/driver-status/DriverSessionAccess';
import { MidiAccess } from '../features/midi/MidiAccess';
import { RuntimeConnection } from '../features/runtime-connection/RuntimeConnection';
import { UpdateAccess } from '../features/updates/UpdateAccess';
import { StudioScreen } from '../screens/StudioScreen';
import { localPorts } from './localPorts';

/** Omitted ports are this environment's own (app/localPorts); tests and stories pass fakes. */
export function App({
  controlPort = localPorts().controlPort,
  updatesPort = localPorts().updatesPort,
  driverPort = localPorts().driverPort,
  runtimePort = localPorts().runtimePort,
  diagnosticsPort = localPorts().diagnosticsPort,
  midiPort = localPorts().midiPort,
}: {
  midiPort?: MidiPort | null;
  controlPort?: ControlPort;
  updatesPort?: UpdatesPort | null;
  driverPort?: DriverPort | null;
  runtimePort?: RuntimePort | null;
  diagnosticsPort?: DiagnosticsPort | null;
}) {
  return (
    <MidiAccess port={midiPort}>
      <GraphApp
        controlPort={controlPort}
        updatesPort={updatesPort}
        driverPort={driverPort}
        runtimePort={runtimePort}
        diagnosticsPort={diagnosticsPort}
      />
    </MidiAccess>
  );
}

function GraphApp({
  controlPort,
  updatesPort,
  driverPort,
  runtimePort,
  diagnosticsPort,
}: {
  controlPort: ControlPort;
  updatesPort: UpdatesPort | null;
  driverPort: DriverPort | null;
  runtimePort: RuntimePort | null;
  diagnosticsPort: DiagnosticsPort | null;
}) {
  const workspace = (openDriverInstaller: (() => void) | undefined, action: React.ReactNode) => (
    <DriverSessionAccess port={driverPort}>
      {(driverSession) => (
        <AboutAccess>
          {(applicationInfo) => (
            <RuntimeGate port={runtimePort} onInstall={openDriverInstaller}>
              <StudioScreen
                applicationInfo={applicationInfo}
                applicationActions={action}
                controlPort={controlPort}
                driverSession={driverSession}
                diagnosticsPort={diagnosticsPort}
                onInstallDriver={openDriverInstaller}
              />
            </RuntimeGate>
          )}
        </AboutAccess>
      )}
    </DriverSessionAccess>
  );
  return (
    <UpdateAccess port={updatesPort}>
      {(action, openDriverInstaller) => workspace(openDriverInstaller, action)}
    </UpdateAccess>
  );
}

function RuntimeGate({
  port,
  onInstall,
  children,
}: {
  port: RuntimePort | null;
  onInstall?: () => void;
  children: React.ReactNode;
}) {
  return port ? (
    <RuntimeConnection port={port} onInstall={onInstall}>
      {children}
    </RuntimeConnection>
  ) : (
    children
  );
}
