import type { ComponentProps, ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { useMidiMappingActive } from '../core/midi/MidiContext';
import { ApplicationStatusBar } from '../core/ui/ApplicationStatusBar';
import { AudioPauseControl } from '../features/audio-session/AudioPauseControl';
import { AudioSessionNotices } from '../features/audio-session/AudioSessionNotices';
import type { DriverSession } from '../features/driver-status/useDriverSession';
import type { useProcessorCatalog } from '../features/routing-workspace/useProcessorCatalog';
import { diagnosticsCount, StudioDiagnostics } from './StudioDiagnostics';
import styles from './StudioScreen.module.css';
import { diagnosticsAnnouncement } from './studioRules';

export function StudioStatusBar({
  applicationInfo,
  processorCatalog,
  driverSession,
  problems,
  problemsRef,
  diagnosticsPort,
  ...props
}: ComponentProps<typeof AudioPauseControl> &
  Pick<ComponentProps<typeof StudioDiagnostics>, 'problems' | 'problemsRef' | 'diagnosticsPort'> & {
    applicationInfo?: ReactNode;
    processorCatalog: ReturnType<typeof useProcessorCatalog>;
    driverSession?: DriverSession;
  }) {
  const { t } = useTranslation();
  return (
    <div className={styles.statusBar} inert={useMidiMappingActive()} data-midi-inactive>
      <ApplicationStatusBar
        information={applicationInfo}
        announcement={diagnosticsAnnouncement(t, diagnosticsCount(problems, processorCatalog))}
        controls={
          <>
            <AudioSessionNotices status={props.status} />
            <AudioPauseControl {...props} />
            <StudioDiagnostics
              status={props.status}
              catalog={processorCatalog}
              driverSession={driverSession}
              problems={problems}
              problemsRef={problemsRef}
              diagnosticsPort={diagnosticsPort}
            />
          </>
        }
      />
    </div>
  );
}
