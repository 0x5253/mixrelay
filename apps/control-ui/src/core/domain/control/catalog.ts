import type { RuntimeGraphDocument } from './graph';

export interface GraphCatalog {
  layout?: GraphLayout;
  layoutRevision?: number;
  protocolVersion: number;
  catalogRevision: number;
  activeId: string;
  graphs: { id: string; name: string }[];
  document: RuntimeGraphDocument;
  persistenceError?: string;
}

/** Presentation-only metadata. Saving it must never submit or reprepare the audio graph. */
export interface GraphLayout {
  /** Display-only labels keyed by UI instance, including split endpoint cards. */
  nodeLabels?: Record<string, string>;
  positions: Record<string, { x: number; y: number }>;
  sizes?: Record<string, { width: number; height: number }>;
  viewport?: { x: number; y: number; zoom: number };
}

export interface GraphLayoutResponse {
  graphId: string;
  layoutRevision: number;
  layout: GraphLayout;
}

export interface GraphLayoutRequest {
  expectedLayoutRevision: number;
  layout: GraphLayout;
}

export type GraphCatalogAction =
  | { action: 'create'; name: string }
  | { action: 'select'; id: string }
  | { action: 'delete'; id: string }
  | { action: 'undo_delete' }
  | { action: 'redo_delete' }
  | { action: 'rename'; id: string; name: string };

export type GraphCatalogRequest = GraphCatalogAction & {
  expectedRevision: number;
  expectedCatalogRevision: number;
};

export type WorkspaceFileAction = 'new' | 'open' | 'save' | 'save_as';

export interface WorkspaceFileRequest {
  action: WorkspaceFileAction;
  expectedRevision: number;
  expectedCatalogRevision: number;
  locale: string;
}
