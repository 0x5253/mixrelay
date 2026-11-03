import type { GraphLayout, GraphLayoutResponse } from '../domain/control/catalog';
import type { RuntimeGraphDocument } from '../domain/control/graph';

export interface WorkspaceView<Model> {
  getGraphId?: () => string | undefined;
  getCatalogRevision?: () => number | undefined;
  hasFailedProcessor?: (graphId: string, nodeId: string, instanceId: string) => boolean;
  beforeApply?: () => Promise<void>;
  beforeReload?: () => Promise<void>;
  getReloadLayout?: (graphId: string) => Promise<GraphLayoutResponse>;
  getDocument: () => RuntimeGraphDocument | null;
  getModel: () => Model | null;
  document: (document: RuntimeGraphDocument) => void;
  restore: (document: RuntimeGraphDocument, savedLayout?: GraphLayout) => void;
  rebase?: (before: RuntimeGraphDocument, current: RuntimeGraphDocument, local: Model) => Model;
  reconcile?: (before: RuntimeGraphDocument, after: RuntimeGraphDocument) => boolean;
  permissionWaiting?: (waiting: boolean) => void;
  state: (state: 'active' | 'applying' | 'blocked') => void;
  error: (code: string | null) => void;
  persistenceWarning?: (code: string | null) => void;
}
