import { useContext, useEffect } from 'react';
import type { GraphCatalog, GraphCatalogAction } from '../../core/domain/control/catalog';
import { MidiContext } from '../../core/midi/MidiContext';

interface GraphSelection {
  catalog: GraphCatalog | null;
  busy: boolean;
  uncertain: boolean;
  error: string | null;
  mutate(action: GraphCatalogAction): Promise<void>;
}

export function useMidiGraphSelection(collection: GraphSelection) {
  const graphId = useContext(MidiContext)?.selected?.target.graphId;
  const { catalog, busy, uncertain, error, mutate } = collection;
  useEffect(() => {
    if (
      graphId &&
      catalog &&
      graphId !== catalog.activeId &&
      catalog.graphs.some((graph) => graph.id === graphId) &&
      !busy &&
      !uncertain &&
      !error
    ) {
      void mutate({ action: 'select', id: graphId });
    }
  }, [graphId, catalog, busy, uncertain, error, mutate]);
}
