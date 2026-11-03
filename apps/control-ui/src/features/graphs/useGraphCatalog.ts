import { useEffect, useMemo, useState, useSyncExternalStore } from 'react';
import { monotonicClock, timerScheduler } from '../../core/adapters/browser/timers';
import type { MonotonicClock, Scheduler } from '../../core/ports/Scheduler';
import { type CatalogPort, GraphCatalogController } from './application/GraphCatalogController';

export function useGraphCatalog(
  controlPort: CatalogPort,
  scheduler: Scheduler = timerScheduler,
  clock: MonotonicClock = monotonicClock,
) {
  const controller = useMemo(
    () => new GraphCatalogController(controlPort, scheduler, clock),
    [controlPort, scheduler, clock],
  );
  const state = useSyncExternalStore(
    controller.subscribe,
    controller.snapshot,
    controller.snapshot,
  );
  useEffect(() => {
    void controller.reload();
    return controller.dispose;
  }, [controller]);
  return {
    ...state,
    mutate: controller.mutate,
    workspaceFile: controller.workspaceFile,
    reload: controller.reload,
    clearError: controller.clearError,
  };
}

export function useCatalogTitle(
  collection: ReturnType<typeof useGraphCatalog>,
  busy: boolean,
  fallbackName: string,
  scheduler: Scheduler = timerScheduler,
) {
  const selected = collection.catalog?.graphs.find(
    (graph) => graph.id === collection.catalog?.activeId,
  );
  const name = selected?.name ?? '';
  const [draft, setDraft] = useState(name);
  useEffect(() => setDraft(name), [name]);
  const normalized = draft.trim() || fallbackName;
  const unsaved = !!selected && normalized !== name;
  useEffect(() => {
    if (!selected || !unsaved || busy || collection.error) return;
    const timer = scheduler.after(500, () => {
      void collection.mutate({ action: 'rename', id: selected.id, name: normalized });
    });
    return () => timer?.();
  }, [selected, unsaved, busy, collection.error, collection.mutate, normalized, scheduler]);
  return {
    draft,
    unsaved,
    setDraft: (value: string) => {
      setDraft(value);
      collection.clearError();
    },
  };
}
