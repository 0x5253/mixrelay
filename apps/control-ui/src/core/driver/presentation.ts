import type { DriverState } from '../domain/control/status';
import type { StatusTone } from '../ui/StatusBadge';

// The workspace and diagnostic card must give the same state the same meaning.
// Transport failure is not an installation failure or an OS permission block.
export const driverPresentation: Record<
  DriverState,
  { action: 'actionInstall' | 'actionRestart' | 'actionDiagnose'; label: string; tone: StatusTone }
> = {
  not_installed: { action: 'actionInstall', label: 'notInstalled', tone: 'neutral' },
  installing: { action: 'actionDiagnose', label: 'installing', tone: 'neutral' },
  restart_required: { action: 'actionRestart', label: 'restartRequired', tone: 'warning' },
  installed_not_loaded: { action: 'actionDiagnose', label: 'installedNotLoaded', tone: 'warning' },
  blocked: { action: 'actionDiagnose', label: 'blocked', tone: 'critical' },
  unavailable: { action: 'actionDiagnose', label: 'unavailable', tone: 'critical' },
  invalid_permissions: { action: 'actionDiagnose', label: 'invalidPermissions', tone: 'critical' },
  version_mismatch: { action: 'actionDiagnose', label: 'versionMismatch', tone: 'warning' },
  ready: { action: 'actionDiagnose', label: 'ready', tone: 'positive' },
};

/** A newer local engine may report a state this UI does not know yet. Fail visibly. */
export function getDriverPresentation(state: string) {
  return Object.hasOwn(driverPresentation, state)
    ? driverPresentation[state as DriverState]
    : driverPresentation.unavailable;
}
