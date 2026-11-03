import { useTranslation } from 'react-i18next';
import type { MixRelayStatus } from '../../core/domain/control/status';
import { AudioSessionAlerts } from './AudioSessionSettings';

/** Keep recording and session failures visible without a global buffer control. */
export function AudioSessionNotices({ status }: { status: MixRelayStatus | null }) {
  const { t } = useTranslation();
  const session = status?.audioSession;
  return (
    <AudioSessionAlerts
      messages={[
        status?.recorderError ? `${t('recorder.failed')}: ${status.recorderError}` : '',
        session?.issue ? t(`audioSession.issues.${session.issue}`) : '',
        session?.endpointPreferencesUnavailable
          ? t('audioSession.issues.endpoint_preferences_unavailable')
          : '',
      ]}
    />
  );
}
