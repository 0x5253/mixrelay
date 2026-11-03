import type { TFunction } from 'i18next';
import type { AudioSessionStatus, MixRelayStatus } from '../domain/control/status';
import { getDriverPresentation } from '../driver/presentation';
import type { StatusTone } from '../ui/StatusBadge';

export interface AudioHealthPresentation {
  label: string;
  tone: StatusTone;
}

/** A running driver alone is not evidence that the audio processing owner is healthy. */
export function getAudioSessionPresentation(
  session: AudioSessionStatus | undefined,
  t: TFunction,
): AudioHealthPresentation {
  if (session?.processingState === 'stopped' || session?.issue === 'processing_failed')
    return {
      label: t(
        `routing.nodeStatus.${session.issue === 'processing_failed' ? 'failed' : 'stopped'}`,
      ),
      tone: 'critical',
    };
  if (!session?.processingState) return { label: t('routing.nodeStatus.unknown'), tone: 'warning' };
  // Session issues are aggregate evidence, not proof that every endpoint is broken.
  if (session.issue || session.endpointPreferencesUnavailable)
    return {
      label: t(`routing.nodeStatus.issues.${session.issue ?? 'preferences_unavailable'}`),
      tone: 'warning',
    };
  // An idle session is ready and healthy: it has nothing to process, not a fault.
  return { label: t(`routing.nodeStatus.${session.processingState}`), tone: 'positive' };
}

export function getEnginePresentation(status: MixRelayStatus | null, t: TFunction) {
  const session = getAudioSessionPresentation(status?.audioSession, t);
  if (!status || session.tone === 'critical') return session;
  if (status.driverState !== 'ready') {
    const driver = getDriverPresentation(status.driverState);
    return {
      label: t(`driver.states.${driver.label}`),
      tone: driver.tone === 'neutral' ? ('warning' as const) : driver.tone,
    };
  }
  return session;
}
