import { useTranslation } from 'react-i18next';
import type { MixRelayStatus } from '../../core/domain/control/status';
import styles from './AudioSessionControl.module.css';
import {
  AudioSessionAlerts,
  AudioSessionSettings,
  type AudioSessionSettingsProps,
} from './AudioSessionSettings';

export function AudioSessionControl({
  status,
  ...settings
}: { status: MixRelayStatus | null } & Omit<AudioSessionSettingsProps, 'session'>) {
  const { t } = useTranslation();
  const session = status?.audioSession;
  const recorderError = status?.recorderError
    ? `${t('recorder.failed')}: ${status.recorderError}`
    : '';
  if (!session)
    return (
      <>
        <span>{t('audioSession.unavailable')}</span>
        <AudioSessionAlerts messages={[recorderError]} />
      </>
    );
  const { capabilities, issue, format } = session;
  const alerts = [
    recorderError,
    issue ? t(`audioSession.issues.${issue}`) : '',
    session.endpointPreferencesUnavailable
      ? t('audioSession.issues.endpoint_preferences_unavailable')
      : '',
  ];
  const duration =
    Number.isFinite(format.sampleRate) &&
    format.sampleRate > 0 &&
    Number.isFinite(format.blockFrames) &&
    format.blockFrames > 0
      ? `${((format.blockFrames / format.sampleRate) * 1000).toFixed(2)} ms`
      : '—';
  return (
    <section className={styles.content} aria-label={t('studio.audioEngine')}>
      <AudioSessionSettings
        {...settings}
        disabled={settings.disabled || !capabilities.canReconfigureLive}
        session={session}
        sessionAlerts={alerts}
      />
      <span className={styles.latency} title={t('audioSession.bufferLatencyHint')}>
        {t('audioSession.bufferLatency')} {duration}
      </span>
    </section>
  );
}
