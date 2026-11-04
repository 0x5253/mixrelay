import type { TFunction } from 'i18next';
import {
  type ComponentProps,
  type FocusEvent,
  type ReactNode,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from 'react';
import { useTranslation } from 'react-i18next';
import { browserProcessorPaths } from '../core/adapters/browser/processorPaths';
import { StudioSession } from '../core/application/workspace/StudioSession';
import type { RuntimeGraphDocument } from '../core/domain/control/graph';
import { workspaceSessionDocument } from '../core/domain/graph/WorkspaceDocuments';
import { MidiContext, MidiPanelContext } from '../core/midi/MidiContext';
import type { ControlPort } from '../core/ports/control/ControlPort';
import type { DiagnosticsPort } from '../core/ports/DiagnosticsPort';
import { useNativeGraphListCommand } from '../core/shell/useNativeGraphListCommand';
import { shellBreakpoints } from '../core/theme/tokens';
import { ActionButton } from '../core/ui/ActionButton';
import { ApplicationStatusBar } from '../core/ui/ApplicationStatusBar';
import { ProductHeading } from '../core/ui/BrandIdentity';
import {
  hasPlatformModifier,
  isNativeTextEntryTarget,
  platformModifier,
} from '../core/ui/platformShortcut';
import { SidebarToggle } from '../core/ui/SidebarToggle';
import { ThemePicker } from '../core/ui/ThemePicker';
import { AudioPauseControl } from '../features/audio-session/AudioPauseControl';
import { AudioSessionNotices } from '../features/audio-session/AudioSessionNotices';
import { useAudioStatus } from '../features/audio-session/useAudioStatus';
import { useSettledAudioStatus } from '../features/audio-session/useSettledAudioStatus';
import { DriverInstallNotice } from '../features/driver-status/DriverInstallNotice';
import type { DriverSession } from '../features/driver-status/useDriverSession';
import { GraphCreateDialog } from '../features/graphs/GraphCreateDialog';
import { GraphList } from '../features/graphs/GraphList';
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
import { renderRecorderNode } from './recorderNodeView';
import { diagnosticsCount, StudioDiagnostics } from './StudioDiagnostics';
import styles from './StudioScreen.module.css';

function useCatalogError(error: string | null) {
  const { t } = useTranslation();
  if (!error) return null;
  if (error === 'graph_catalog_unavailable') return t('studio.catalogUnavailable');
  return t(`routing.errors.${error}`, { defaultValue: t('routing.errors.control_request_failed') });
}

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
        event.defaultPrevented ||
        event.altKey ||
        isNativeTextEntryTarget(event.target) ||
        !hasPlatformModifier(event) ||
        !event.shiftKey ||
        event.key.toLowerCase() !== 'n'
      ) {
        return;
      }
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
  const catalogErrorText = useCatalogError(collection.error);
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
          error={catalogErrorText}
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

function StudioFrame({
  children,
  applicationActions,
  ...sidebar
}: Omit<ComponentProps<typeof GraphSidebar>, 'hidden'> & {
  children: ReactNode;
  applicationActions?: ReactNode;
}) {
  const overlay = useOverlaySidebar();
  const midiPanel = useContext(MidiPanelContext);
  const mapping = useContext(MidiContext)?.active ?? false;
  // Carbon UI shell: wide windows keep the list fixed (View menu may hide it); narrow
  // windows collapse it behind the header menu button and open it over the canvas.
  const [fixedVisible, setFixedVisible] = useState(true);
  const [overlayOpen, setOverlayOpen] = useState(false);
  const sidebarOpen = overlay ? overlayOpen : fixedVisible;
  const shell = useRef<HTMLElement>(null);
  const pendingSelection = useRef<string | null>(null);
  // A removed or newly inactive control hands focus to the surviving shell control.
  const focusOwner = useRef<HTMLElement | null>(null);
  const trackFocus = (event: FocusEvent<HTMLElement>) => {
    focusOwner.current = event.target;
  };
  // Focus that moves elsewhere (a dialog, another control) is no longer the shell's. A
  // plain blur of a control that stays visible is the user leaving it, not a hide.
  const leaveFocus = (event: FocusEvent<HTMLElement>) => {
    const next = event.relatedTarget;
    if (next) {
      if (!event.currentTarget.contains(next)) focusOwner.current = null;
      return;
    }
    const blurred = event.target;
    queueMicrotask(() => {
      if (focusOwner.current === blurred && shown(blurred) && !shown(document.activeElement))
        focusOwner.current = null;
    });
  };
  const closeOverlay = useCallback(() => setOverlayOpen(false), []);
  // Entering the overlay width closes the list so it never starts over the canvas.
  useEffect(() => {
    if (overlay) closeOverlay();
  }, [overlay, closeOverlay]);
  const toggleSidebar = useCallback(() => {
    if (mapping) return;
    if (overlay) setOverlayOpen((open) => !open);
    else setFixedVisible((visible) => !visible);
  }, [overlay, mapping]);
  useNativeGraphListCommand(sidebarOpen, toggleSidebar);
  // An overlay closes only once the chosen graph is active. A request that settles without
  // activating it (failure, or another graph became active) ends the intent, so the
  // sidebar and its error stay visible and a later refresh cannot reuse it.
  const activeId = sidebar.collection.catalog?.activeId;
  const catalogBusy = sidebar.collection.busy;
  const catalogError = sidebar.collection.error;
  useEffect(() => {
    const pending = pendingSelection.current;
    if (pending === null) return;
    if (pending === activeId) {
      pendingSelection.current = null;
      if (overlay) closeOverlay();
    } else if (!catalogBusy || catalogError) {
      pendingSelection.current = null;
    }
  }, [activeId, catalogBusy, catalogError, overlay, closeOverlay]);
  // biome-ignore lint/correctness/useExhaustiveDependencies: Runs after each list visibility or breakpoint change, which is what can remove the focused control.
  useEffect(() => {
    const owner = focusOwner.current;
    if (!owner) return;
    // Never take focus from a control that still holds it (including a dialog).
    if (shown(document.activeElement)) return;
    focusOwner.current = null;
    // A still-visible owner means the user left it on purpose; only a hidden or removed
    // owner hands focus to the surviving control.
    if (shown(owner)) return;
    const next = mapping
      ? shell.current?.querySelector<HTMLElement>('[data-midi-toggle]')
      : surviving(shell.current);
    next?.focus();
  }, [sidebarOpen, overlay, mapping]);
  return (
    <main
      ref={shell}
      className={styles.shell}
      data-sidebar-open={mapping || sidebarOpen}
      onFocus={trackFocus}
      onBlur={leaveFocus}
    >
      <StudioApplicationHeader
        overlay={overlay}
        mapping={mapping}
        sidebarOpen={sidebarOpen}
        toggleSidebar={toggleSidebar}
      >
        {applicationActions}
      </StudioApplicationHeader>
      {mapping ? (
        <div className={styles.sidebar}>{midiPanel}</div>
      ) : (
        <GraphSidebar
          hidden={!sidebarOpen}
          {...sidebar}
          onSelect={(id) => {
            pendingSelection.current = id;
            sidebar.onSelect(id);
          }}
        />
      )}
      {children}
    </main>
  );
}

/** Whether a connected control can still hold focus in the current shell state. */
function shown(element: Element | null): boolean {
  return Boolean(
    element &&
      element !== document.body &&
      element.isConnected &&
      !element.closest('[hidden], [inert]') &&
      !element.matches(':disabled'),
  );
}

/** The first shell control still able to take focus: menu button, list, workspace, header. */
function surviving(root: HTMLElement | null): HTMLElement | undefined {
  return [
    '[aria-controls="graph-sidebar"]',
    '#graph-sidebar [aria-current="page"]:not(:disabled), #graph-sidebar button:not(:disabled)',
    '#studio-workspace',
    'header button:not(:disabled)',
  ]
    .map((selector) => root?.querySelector<HTMLElement>(selector))
    .find((element): element is HTMLElement => element?.closest('[inert], [hidden]') === null);
}

/** Always present, so a change to zero is read as well as a new count. */
function diagnosticsAnnouncement(t: TFunction, count: number) {
  return count ? t('studio.diagnosticsAnnouncement', { count }) : t('studio.diagnosticsClear');
}

function StudioStatusBar({
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
    <div className={styles.statusBar} inert={useContext(MidiContext)?.active} data-midi-inactive>
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

function GraphSidebar({
  hidden,
  collection,
  disabled,
  onCreate,
  onSelect,
}: {
  hidden: boolean;
  collection: ReturnType<typeof useGraphCatalog>;
  disabled: boolean;
  onCreate: () => void;
  onSelect: (id: string) => void;
}) {
  const { t } = useTranslation();
  const { catalog, busy: catalogBusy, error: catalogError, reload: reloadCatalog } = collection;
  const catalogErrorText = useCatalogError(catalogError);
  return (
    <aside
      onKeyDown={(event) => {
        if (
          disabled ||
          !hasPlatformModifier(event) ||
          event.altKey ||
          isNativeTextEntryTarget(event.target) ||
          event.key.toLowerCase() !== 'z'
        )
          return;
        event.preventDefault();
        event.stopPropagation();
        void collection.mutate({ action: event.shiftKey ? 'redo_delete' : 'undo_delete' });
      }}
      id="graph-sidebar"
      className={styles.sidebar}
      aria-label={t('studio.graphs')}
      hidden={hidden}
    >
      <nav className={styles.graphList} aria-label={t('studio.graphs')}>
        {catalog ? (
          <GraphList
            catalog={catalog}
            disabled={disabled}
            busy={catalogBusy}
            error={catalogErrorText}
            onSelect={onSelect}
            onDelete={(id) => {
              if (!disabled) void collection.mutate({ action: 'delete', id });
            }}
          />
        ) : null}
        {catalogError ? (
          <div role="alert">
            <p>{catalogErrorText}</p>
            <ActionButton disabled={catalogBusy} onClick={() => void reloadCatalog()}>
              {t('studio.reloadGraphs')}
            </ActionButton>
          </div>
        ) : null}
        {catalog?.persistenceError ? <p role="alert">{t('studio.graphSaveFailed')}</p> : null}
        <div className={styles.sidebarFooter}>
          <ActionButton
            density="compact"
            stretch
            align="start"
            aria-keyshortcuts={`${platformModifier()}+Shift+N`}
            disabled={!catalog}
            onClick={onCreate}
          >
            {/* One inline run: the flex button would drop a space between children. */}
            <span>
              <span aria-hidden="true">+ </span>
              {t('studio.newGraph')}
            </span>
          </ActionButton>
        </div>
      </nav>
    </aside>
  );
}

function driverRoutingActions(session: DriverSession | undefined) {
  return {
    settings: session?.snapshot?.settings,
    enabled: session?.snapshot?.state === 'ready' && !session.busy,
    error: session?.error ? { endpointId: session.endpointId, message: session.error } : null,
    change: (
      endpointId: string,
      update: import('../features/routing-workspace/virtualEndpointNodes').VirtualEndpointUpdate,
    ) => {
      void session?.changeDevice(endpointId, update);
    },
    create: async (spec: import('../core/domain/control/status').VirtualEndpointSpec) => {
      const snapshot = await session?.mutate({ action: 'create', spec });
      return snapshot?.settings.devices.find((device) => device.spec.id === spec.id)?.spec ?? null;
    },
  };
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

const overlayQuery = `(max-width: ${shellBreakpoints.overlaySidebar}px)`;

/** Whether the viewport is narrow enough for the sidebar to overlay the canvas. */
function useOverlaySidebar() {
  const [overlay, setOverlay] = useState(() => window.matchMedia(overlayQuery).matches);
  useEffect(() => {
    const query = window.matchMedia(overlayQuery);
    const update = () => setOverlay(query.matches);
    update();
    query.addEventListener('change', update);
    return () => query.removeEventListener('change', update);
  }, []);
  return overlay;
}

function StudioApplicationHeader({
  overlay,
  mapping,
  sidebarOpen,
  toggleSidebar,
  children,
}: {
  overlay: boolean;
  mapping: boolean;
  sidebarOpen: boolean;
  toggleSidebar: () => void;
  children: ReactNode;
}) {
  const { t } = useTranslation();
  return (
    <header
      className={styles.applicationHeader}
      inert={mapping}
      data-midi-inactive={mapping || undefined}
    >
      <span className={styles.applicationLead}>
        {overlay && !mapping ? (
          <SidebarToggle
            label={t('studio.graphs')}
            opened={sidebarOpen}
            controls="graph-sidebar"
            onToggle={toggleSidebar}
          />
        ) : null}
        <ProductHeading />
      </span>
      <span key={String(mapping)} className={styles.applicationActions}>
        <ThemePicker />
        {children}
      </span>
    </header>
  );
}
