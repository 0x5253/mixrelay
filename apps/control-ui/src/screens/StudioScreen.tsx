import {
  type ReactNode,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  useSyncExternalStore,
} from 'react';
import { useTranslation } from 'react-i18next';
import { browserProcessorPaths } from '../core/adapters/browser/processorPaths';
import { StudioSession } from '../core/application/workspace/StudioSession';
import type { RuntimeGraphDocument } from '../core/domain/control/graph';
import { workspaceSessionDocument } from '../core/domain/graph/WorkspaceDocuments';
import { MidiContext } from '../core/midi/MidiContext';
import type { ControlPort } from '../core/ports/control/ControlPort';
import type { DiagnosticsPort } from '../core/ports/DiagnosticsPort';
import { hasPlatformModifier, isNativeTextEntryTarget } from '../core/ui/platformShortcut';
import { useAudioStatus } from '../features/audio-session/useAudioStatus';
import { useSettledAudioStatus } from '../features/audio-session/useSettledAudioStatus';
import { DriverInstallNotice } from '../features/driver-status/DriverInstallNotice';
import type { DriverSession } from '../features/driver-status/useDriverSession';
import { GraphCreateDialog } from '../features/graphs/GraphCreateDialog';
import { useCatalogTitle, useGraphCatalog } from '../features/graphs/useGraphCatalog';
import { WorkspaceFileMenu } from '../features/graphs/WorkspaceFileMenu';
import { MidiSettingsButton } from '../features/midi/MidiAccess';
import { useMidiGraphSelection } from '../features/midi/useMidiGraphSelection';
import { RoutingWorkspace } from '../features/routing-workspace/RoutingWorkspace';
import { useProcessorCatalog } from '../features/routing-workspace/useProcessorCatalog';
import {
  noWorkspaceProblems,
  type WorkspaceProblemSummary,
} from '../features/routing-workspace/WorkspaceProblems';
import { driverRoutingActions } from './driverRoutingActions';
import { renderRecorderNode } from './recorderNodeView';
import { StudioFrame } from './StudioFrame';
import styles from './StudioScreen.module.css';
import { StudioStatusBar } from './StudioStatusBar';
import { catalogErrorText, isNewGraphShortcut } from './studioRules';

function useOwnedSessionDocument(controlPort: ControlPort, catalogDocument?: RuntimeGraphDocument) {
  const session = useMemo(
    () => new StudioSession(controlPort, catalogDocument),
    [controlPort, catalogDocument],
  );
  const document = useSyncExternalStore(session.subscribe, session.snapshot, session.snapshot);
  return [document, session.accept] as const;
}

function useNewGraphShortcut(enabled: boolean, open: () => void) {
  useEffect(() => {
    const handle = (event: KeyboardEvent) => {
      if (
        !isNewGraphShortcut(
          event,
          hasPlatformModifier(event),
          isNativeTextEntryTarget(event.target),
        )
      )
        return;
      event.preventDefault();
      if (enabled) open();
    };
    window.addEventListener('keydown', handle);
    return () => window.removeEventListener('keydown', handle);
  }, [enabled, open]);
}

function routingWorkspaceProps(
  status: ReturnType<typeof useAudioStatus>,
  catalog: ReturnType<typeof useGraphCatalog>['catalog'],
) {
  return {
    headerEnd: <MidiSettingsButton />,
    renderRecorder: renderRecorderNode,
    audioSession: status?.audioSession,
    processorTimings: status?.processorTimings,
    failedProcessors: status?.failedProcessors,
    catalogRevision: catalog?.catalogRevision,
    playbackPaused: status?.enginePower?.enabled === false || status?.enginePower?.transitioning,
  };
}

/** Status reloads after a session apply so the bar reflects the applied document. */
function useAppliedStatus(
  controlPort: ControlPort,
  setSessionDocument: (document: RuntimeGraphDocument) => void,
) {
  const [statusRefresh, setStatusRefresh] = useState(0);
  const status = useSettledAudioStatus(useAudioStatus(controlPort, statusRefresh));
  const onSessionApplied = (document: RuntimeGraphDocument) => {
    setSessionDocument(document);
    setStatusRefresh((value) => value + 1);
  };
  return { status, onSessionApplied };
}

/** Workspace problems render inside the diagnostics window and count on its status button. */
function useProblemChannel() {
  const [problems, setProblems] = useState<WorkspaceProblemSummary>(noWorkspaceProblems);
  const [target, setTarget] = useState<HTMLDivElement | null>(null);
  return {
    workspace: { problemsTarget: target, onProblemsChange: setProblems },
    statusBar: { problems, problemsRef: setTarget },
  };
}

interface StudioScreenProps {
  controlPort: ControlPort;
  applicationActions?: ReactNode;
  applicationInfo?: ReactNode;
  driverSession?: DriverSession;
  diagnosticsPort?: DiagnosticsPort | null;
  onInstallDriver?: () => void;
}

export function StudioScreen({
  controlPort,
  applicationActions,
  applicationInfo,
  driverSession,
  diagnosticsPort,
  onInstallDriver,
}: StudioScreenProps) {
  const { t } = useTranslation();
  const mapping = useContext(MidiContext)?.active ?? false;
  const collection = useGraphCatalog(controlPort);
  useMidiGraphSelection(collection);
  const processorCatalog = useProcessorCatalog(controlPort, undefined, browserProcessorPaths);
  const problems = useProblemChannel();
  const catalogError = catalogErrorText(t, collection.error);
  const { catalog, busy: catalogBusy, uncertain: catalogUncertain } = collection;
  const [sessionBusy, setSessionBusy] = useState(false);
  const [workspaceBusy, setWorkspaceBusy] = useState(false);
  const ownedSession = useOwnedSessionDocument(controlPort, collection.document);
  const [sessionDocument, setSessionDocument] = ownedSession;
  const { status, onSessionApplied } = useAppliedStatus(controlPort, setSessionDocument);
  const { isCreatingGraph, setIsCreatingGraph, openNewGraph } = useGraphCreation(
    catalog?.activeId,
    !mapping,
  );
  const busy = catalogBusy || sessionBusy || workspaceBusy || catalogUncertain;
  const title = useCatalogTitle(collection, busy, t('studio.untitledGraph'));
  const controlsDisabled = busy || title.unsaved || mapping;
  const mutateCatalog = busy || mapping ? () => undefined : collection.mutate;
  const documentForWorkspace = workspaceSessionDocument(
    sessionDocument,
    catalog?.document.revision,
    collection.document,
  );

  return (
    <StudioFrame
      applicationActions={applicationActions}
      collection={collection}
      disabled={controlsDisabled}
      onCreate={openNewGraph}
      onSelect={(id) => void mutateCatalog({ action: 'select', id })}
    >
      <section
        id="studio-workspace"
        tabIndex={-1}
        className={styles.workspace}
        aria-label={t('studio.workspace')}
        inert={sessionBusy || catalogBusy || catalogUncertain}
      >
        <RoutingWorkspace
          processorCatalog={processorCatalog}
          notice={driverInstallNotice(driverSession, onInstallDriver)}
          {...problems.workspace}
          headerActions={
            <WorkspaceFileMenu
              controlPort={controlPort}
              disabled={controlsDisabled}
              run={collection.workspaceFile}
            />
          }
          deviceDriverAction={driverRoutingActions(driverSession)}
          key={`${catalog?.activeId ?? 'engine'}:${collection.documentGeneration}`}
          graphId={catalog?.activeId}
          interactionDisabled={catalogBusy || catalogUncertain || sessionBusy || mapping}
          {...routingWorkspaceProps(status, catalog)}
          sessionDocument={documentForWorkspace}
          controlPort={controlPort}
          onBusyChange={setWorkspaceBusy}
          onTitleChange={title.setDraft}
          title={catalog ? title.draft : undefined}
        />
      </section>

      <StudioStatusBar
        processorCatalog={processorCatalog}
        driverSession={driverSession}
        applicationInfo={applicationInfo}
        {...problems.statusBar}
        diagnosticsPort={diagnosticsPort}
        disabled={controlsDisabled}
        status={status}
        controlPort={controlPort}
        onApplied={onSessionApplied}
        onBusyChange={setSessionBusy}
      />

      {isCreatingGraph ? (
        <GraphCreateDialog
          busy={controlsDisabled}
          creating={catalogBusy}
          error={catalogError}
          onCancel={() => setIsCreatingGraph(false)}
          onCreate={(name) => void mutateCatalog({ action: 'create', name })}
          suggestedName={t('studio.untitledGraphNumbered', {
            count: (catalog?.graphs.length ?? 0) + 1,
          })}
        />
      ) : null}
    </StudioFrame>
  );
}

function useGraphCreation(activeId: string | undefined, allowed: boolean) {
  const [isCreatingGraph, setIsCreatingGraph] = useState(false);
  useEffect(() => {
    if (activeId) setIsCreatingGraph(false);
  }, [activeId]);
  const openNewGraph = useCallback(() => setIsCreatingGraph(true), []);
  useNewGraphShortcut(Boolean(activeId) && allowed, openNewGraph);
  return { isCreatingGraph, setIsCreatingGraph, openNewGraph };
}

function driverInstallNotice(session: DriverSession | undefined, onInstall?: () => void) {
  return (
    <DriverInstallNotice
      state={session?.snapshot?.state}
      onInstall={onInstall}
      className={styles.driverNotice}
    />
  );
}
