import type { DriverCommand, DriverSnapshot } from '../../domain/driver';
import type { DriverPort } from '../../ports/DriverPort';
import { virtualEndpointSettings } from '../control/protocolDecoders';
import { literal, nonNegativeInteger, nullable, object, optional, string } from '../decode';
import { type NativeInvoke, tauriInvoke } from './nativeInvoke';

const driverSnapshot = object<DriverSnapshot>({
  protocolVersion: literal(1),
  revision: nonNegativeInteger,
  state: literal('ready', 'not_installed', 'installed_not_loaded', 'unavailable'),
  installedVersion: nullable(string),
  settings: virtualEndpointSettings,
  persistenceError: optional(string),
});

export class TauriDriverPort implements DriverPort {
  constructor(private readonly invoke: NativeInvoke = tauriInvoke) {}

  async getSnapshot(): Promise<DriverSnapshot> {
    return driverSnapshot(await this.invoke('driver_status'), 'driver_status');
  }

  async mutate(command: DriverCommand): Promise<DriverSnapshot> {
    return driverSnapshot(await this.invoke('driver_mutate', { command }), 'driver_mutate');
  }
}
