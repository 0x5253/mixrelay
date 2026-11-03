import { type Ref, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { getAudioSessionPresentation } from '../core/audio-session/presentation';
import type { MixRelayStatus } from '../core/domain/control/status';
import { scanFailureCounts } from '../core/domain/diagnostics';
import { getDriverPresentation } from '../core/driver/presentation';
import type { DiagnosticsPort } from '../core/ports/DiagnosticsPort';
import { ActionButton } from '../core/ui/ActionButton';
import { ContentSection } from '../core/ui/ContentSection';
import { Dialog } from '../core/ui/Dialog';
import { Inline } from '../core/ui/Inline';
import { InlineNotice } from '../core/ui/InlineNotice';
import { Stack } from '../core/ui/Stack';
import { StatusIndicator } from '../core/ui/StatusIndicator';
import { Text } from '../core/ui/Text';
import { AudioSessionNotices } from '../features/audio-session/AudioSessionNotices';
import type { DriverSession } from '../features/driver-status/useDriverSession';
import { ProcessorScanReportContent } from '../features/routing-workspace/ProcessorScanReport';
import type { useProcessorCatalog } from '../features/routing-workspace/useProcessorCatalog';
import type { WorkspaceProblemSummary } from '../features/routing-workspace/WorkspaceProblems';

/** Diagnostics collects current problems, runtime state and the support report. Opening it
 * issues no audio or discovery command. */
export function StudioDiagnostics({
  status,
  catalog,
  driverSession,
  problems,
  problemsRef,
  diagnosticsPort,
}: {
  status: MixRelayStatus | null;
  catalog: ReturnType<typeof useProcessorCatalog>;
  driverSession?: DriverSession;
  problems: WorkspaceProblemSummary;
  /** The routing workspace renders its problems and recovery actions here. */
  problemsRef: Ref<HTMLDivElement>;
  diagnosticsPort?: DiagnosticsPort | null;
}) {
  const { t } = useTranslation();
  const [opened, setOpened] = useState(false);
  const scanCount = scanFailureTotal(catalog);
  const count = diagnosticsCount(problems, catalog);
  const hasProblems = problems.codes.length > 0;
  const description = [
    hasProblems ? t('studio.problemsAnnouncement', { count: problems.codes.length }) : null,
    scanCount ? t('routing.processorControl.details', { count: scanCount }) : null,
  ]
    .filter(Boolean)
    .join(' · ');
  return (
    <>
      {problems.tone ? (
        <StatusIndicator
          tone={problems.tone}
          label={t('studio.problemsAnnouncement', { count: problems.codes.length })}
        />
      ) : null}
      <ActionButton
        density="compact"
        aria-haspopup="dialog"
        aria-expanded={opened}
        aria-description={description || undefined}
        onClick={() => setOpened(true)}
      >
        {t('studio.diagnostics')}
        {count ? ` · ${count}` : ''}
      </ActionButton>
      <Dialog
        opened={opened}
        onClose={() => setOpened(false)}
        title={t('studio.diagnostics')}
        closeLabel={t('common.close')}
        size="md"
      >
        <Stack gap="section">
          {hasProblems ? (
            <ContentSection title={t('studio.currentProblems')}>
              <div ref={problemsRef} />
            </ContentSection>
          ) : null}
          <ContentSection title={t('studio.audioEngine')} divider={hasProblems ? 'before' : 'none'}>
            <EngineStatus status={status} />
          </ContentSection>
          <ContentSection title={t('driver.title')} divider="before">
            <DriverStatus status={status} driverSession={driverSession} />
          </ContentSection>
          <ContentSection title={t('routing.processorControl.reportTitle')} divider="before">
            <ProcessorScanReportContent catalog={catalog} />
          </ContentSection>
          {diagnosticsPort ? (
            <ContentSection title={t('studio.support')} divider="before">
              <SupportReport port={diagnosticsPort} problems={problems} catalog={catalog} />
            </ContentSection>
          ) : null}
        </Stack>
      </Dialog>
    </>
  );
}

/** The number the status button shows: current problems plus unique scan failures. */
export function diagnosticsCount(
  problems: WorkspaceProblemSummary,
  catalog: ReturnType<typeof useProcessorCatalog>,
) {
  return problems.codes.length + scanFailureTotal(catalog);
}

function scanFailureTotal(catalog: ReturnType<typeof useProcessorCatalog>) {
  return new Set(catalog.diagnostics.map((item) => JSON.stringify(item))).size;
}

function EngineStatus({ status }: { status: MixRelayStatus | null }) {
  const { t } = useTranslation();
  const engine = getAudioSessionPresentation(status?.audioSession, t);
  const paused = status?.enginePower?.enabled === false;
  return (
    <>
      <Text tone={paused ? 'neutral' : engine.tone}>
        {paused ? t('studio.audioPaused') : engine.label}
      </Text>
      {status?.audioSession && (
        <Text kind="supporting">
          {t('audioSession.backend')}:{' '}
          {t(`audioSession.backends.${status.audioSession.capabilities.backend}`)}
          {' · '}
          {status.audioSession.format.sampleRate} Hz · {status.audioSession.format.blockFrames}{' '}
          {t('audioSession.frames')}
        </Text>
      )}
      <AudioSessionNotices status={status} />
      {status?.enginePower?.pauseReason === 'repeatedFailures' && (
        <Text tone="warning">{t('audioSession.pausedAfterFailures')}</Text>
      )}
      {status?.enginePower?.unsaved && (
        <Text tone="warning">{t('audioSession.pauseNotSaved')}</Text>
      )}
    </>
  );
}

function DriverStatus({
  status,
  driverSession,
}: {
  status: MixRelayStatus | null;
  driverSession?: DriverSession;
}) {
  const { t } = useTranslation();
  const driver = getDriverPresentation(
    driverSession?.snapshot?.state ?? status?.driverState ?? 'unavailable',
  );
  return (
    <>
      <Text tone={driver.tone}>{t(`driver.states.${driver.label}`)}</Text>
      {driverSession?.error && <Text tone="warning">{driverSession.error}</Text>}
    </>
  );
}

/** The native side opens the save dialog; only problem codes and scan counts leave the UI. */
function SupportReport({
  port,
  problems,
  catalog,
}: {
  port: DiagnosticsPort;
  problems: WorkspaceProblemSummary;
  catalog: ReturnType<typeof useProcessorCatalog>;
}) {
  const { t } = useTranslation();
  const [failed, setFailed] = useState(false);
  const save = () => {
    setFailed(false);
    port
      .saveReport({
        problems: [...problems.codes],
        processorScan: scanFailureCounts(catalog.diagnostics),
      })
      .catch(() => setFailed(true));
  };
  return (
    <>
      <Text kind="supporting">{t('studio.supportReport')}</Text>
      <Inline>
        <ActionButton emphasis="outlined" onClick={save}>
          {t('studio.saveReport')}
        </ActionButton>
      </Inline>
      {failed ? <InlineNotice tone="critical">{t('studio.saveReportFailed')}</InlineNotice> : null}
    </>
  );
}
