import { useCallback, useEffect, useRef, useState } from 'react';
import type { ProcessorPaths } from '../../core/ports/ProcessorPaths';
import { CatalogSession } from './application/CatalogSession';
import { initialState, type ProcessorCatalog } from './domain/processorCatalog';
import type { CatalogAction, CatalogPort } from './ports/ProcessorDiscovery';

/** Preserve the last catalog on failure; cancellation never pretends a scan succeeded. */
export function useProcessorCatalog(
  controlPort: CatalogPort,
  suppliedCatalog?: ProcessorCatalog,
  pathsStore?: ProcessorPaths,
): ProcessorCatalog {
  const owned = suppliedCatalog === undefined;
  const [state, setState] = useState(initialState);
  const request = useRef<((action?: CatalogAction) => void) | null>(null);
  const refresh = useCallback(
    (directory: boolean) => request.current?.(directory ? 'directory' : 'scan'),
    [],
  );
  const retry = useCallback(() => request.current?.(), []);
  useEffect(() => {
    if (!owned) return;
    setState(initialState);
    const session = new CatalogSession(controlPort, pathsStore, (patch) =>
      setState((current) => ({ ...current, ...patch })),
    );
    request.current = (action) => {
      void session.run(action);
    };
    void session.run('initial');
    return () => {
      session.dispose();
      request.current = null;
    };
  }, [controlPort, owned, pathsStore]);
  return suppliedCatalog ?? { ...state, refresh, retry };
}
