import type { DriverCommand, DriverSnapshot } from '../domain/driver';

/** Native virtual-device settings and mutations. */
export interface DriverPort {
  getSnapshot(): Promise<DriverSnapshot>;
  mutate(command: DriverCommand): Promise<DriverSnapshot>;
}
