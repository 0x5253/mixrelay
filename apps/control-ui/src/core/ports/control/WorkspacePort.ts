import type {
  GraphCatalog,
  GraphCatalogRequest,
  GraphLayoutRequest,
  GraphLayoutResponse,
  WorkspaceFileAction,
  WorkspaceFileRequest,
} from '../../domain/control/catalog';

/** The saved graph catalog, its presentation layout and workspace files. */
export interface WorkspacePort {
  getGraphCatalog(): Promise<GraphCatalog>;
  mutateGraphCatalog(request: GraphCatalogRequest): Promise<GraphCatalog>;
  getGraphLayout(graphId: string): Promise<GraphLayoutResponse>;
  setGraphLayout(graphId: string, request: GraphLayoutRequest): Promise<GraphLayoutResponse>;
  /** Resolves null when the user cancels the native file dialog. */
  workspaceFile?(request: WorkspaceFileRequest): Promise<GraphCatalog | null>;
  onWorkspaceFile?(listener: (action: WorkspaceFileAction) => void): Promise<() => void>;
}
