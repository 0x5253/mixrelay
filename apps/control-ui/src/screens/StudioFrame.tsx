import {
  type FocusEvent,
  type ReactNode,
  type RefObject,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
} from 'react';
import { useTranslation } from 'react-i18next';
import { MidiContext, MidiPanelContext } from '../core/midi/MidiContext';
import { useNativeGraphListCommand } from '../core/shell/useNativeGraphListCommand';
import { shellBreakpoints } from '../core/theme/tokens';
import { ProductHeading } from '../core/ui/BrandIdentity';
import { SidebarToggle } from '../core/ui/SidebarToggle';
import { ThemePicker } from '../core/ui/ThemePicker';
import { GraphSidebar, type GraphSidebarProps } from './GraphSidebar';
import styles from './StudioScreen.module.css';
import { graphChoiceOutcome } from './studioRules';

/** The Studio shell: application header, graph list or MIDI panel, and the workspace. */
export function StudioFrame({
  children,
  applicationActions,
  ...sidebar
}: Omit<GraphSidebarProps, 'hidden'> & {
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
  const chosenGraph = useRef<string | null>(null);
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
  // An overlay closes only once the chosen graph is active; a choice that settles without
  // activating it ends, so the sidebar and its error stay and a later refresh cannot reuse it.
  const activeId = sidebar.collection.catalog?.activeId;
  const catalogBusy = sidebar.collection.busy;
  const catalogError = sidebar.collection.error;
  useEffect(() => {
    const chosen = chosenGraph.current;
    if (chosen === null) return;
    const outcome = graphChoiceOutcome(chosen, {
      activeId,
      busy: catalogBusy,
      error: catalogError,
    });
    if (outcome === 'pending') return;
    chosenGraph.current = null;
    if (outcome === 'activated' && overlay) closeOverlay();
  }, [activeId, catalogBusy, catalogError, overlay, closeOverlay]);
  const focus = useShellFocus(shell, { sidebarOpen, overlay, mapping });
  return (
    <main
      ref={shell}
      className={styles.shell}
      data-sidebar-open={mapping || sidebarOpen}
      onFocus={focus.track}
      onBlur={focus.leave}
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
            chosenGraph.current = id;
            sidebar.onSelect(id);
          }}
        />
      )}
      {children}
    </main>
  );
}

/** A removed or newly inactive control hands focus to the surviving shell control. */
function useShellFocus(
  shell: RefObject<HTMLElement | null>,
  { sidebarOpen, overlay, mapping }: { sidebarOpen: boolean; overlay: boolean; mapping: boolean },
) {
  const owner = useRef<HTMLElement | null>(null);
  const track = (event: FocusEvent<HTMLElement>) => {
    owner.current = event.target;
  };
  // Focus that moves elsewhere (a dialog, another control) is no longer the shell's. A
  // plain blur of a control that stays visible is the user leaving it, not a hide.
  const leave = (event: FocusEvent<HTMLElement>) => {
    const next = event.relatedTarget;
    if (next) {
      if (!event.currentTarget.contains(next)) owner.current = null;
      return;
    }
    const blurred = event.target;
    queueMicrotask(() => {
      if (owner.current === blurred && shown(blurred) && !shown(document.activeElement))
        owner.current = null;
    });
  };
  // biome-ignore lint/correctness/useExhaustiveDependencies: Runs after each list visibility or breakpoint change, which is what can remove the focused control.
  useEffect(() => {
    const focused = owner.current;
    if (!focused) return;
    // Never take focus from a control that still holds it (including a dialog).
    if (shown(document.activeElement)) return;
    owner.current = null;
    // A still-visible owner means the user left it on purpose; only a hidden or removed
    // owner hands focus to the surviving control.
    if (shown(focused)) return;
    const next = mapping
      ? shell.current?.querySelector<HTMLElement>('[data-midi-toggle]')
      : surviving(shell.current);
    next?.focus();
  }, [sidebarOpen, overlay, mapping]);
  return { track, leave };
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
