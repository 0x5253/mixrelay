import type { TFunction } from 'i18next';

export interface StudioKey {
  key: string;
  altKey: boolean;
  shiftKey: boolean;
}

/** The platform modifier with Shift+N opens the new-graph dialog, except in text entry or
 * when another control already handled the key. */
export function isNewGraphShortcut(
  event: StudioKey & { defaultPrevented: boolean },
  platformModifier: boolean,
  textEntry: boolean,
) {
  return (
    !event.defaultPrevented &&
    !event.altKey &&
    !textEntry &&
    platformModifier &&
    event.shiftKey &&
    event.key.toLowerCase() === 'n'
  );
}

/** In the graph list the platform modifier with Z undoes a graph deletion and with Shift
 * redoes it; text entry keeps its own undo. */
export function graphListHistoryAction(
  event: StudioKey,
  platformModifier: boolean,
  textEntry: boolean,
): 'undo_delete' | 'redo_delete' | null {
  if (!platformModifier || event.altKey || textEntry || event.key.toLowerCase() !== 'z')
    return null;
  return event.shiftKey ? 'redo_delete' : 'undo_delete';
}

/** A graph chosen from the overlay list closes it once active. A request that settles
 * without activating it (failure, or another graph became active) abandons the choice. */
export function graphChoiceOutcome(
  chosenId: string,
  catalog: { activeId: string | undefined; busy: boolean; error: string | null },
): 'activated' | 'abandoned' | 'pending' {
  if (chosenId === catalog.activeId) return 'activated';
  if (!catalog.busy || catalog.error) return 'abandoned';
  return 'pending';
}

export function catalogErrorText(t: TFunction, error: string | null) {
  if (!error) return null;
  if (error === 'graph_catalog_unavailable') return t('studio.catalogUnavailable');
  return t(`routing.errors.${error}`, { defaultValue: t('routing.errors.control_request_failed') });
}

/** Always present, so a change to zero is read as well as a new count. */
export function diagnosticsAnnouncement(t: TFunction, count: number) {
  return count ? t('studio.diagnosticsAnnouncement', { count }) : t('studio.diagnosticsClear');
}
