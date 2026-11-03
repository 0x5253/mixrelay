import type {
  ExternalProcessorDescriptor,
  ExternalProcessorDiagnostic,
} from '../../../core/domain/control/processors';

export type ProcessorCatalogFailure = 'catalog' | 'directory' | 'paths';
export interface CatalogState {
  processors: ExternalProcessorDescriptor[];
  diagnostics: ExternalProcessorDiagnostic[];
  failure: ProcessorCatalogFailure | null;
  busy: boolean;
}
export interface ProcessorCatalog extends CatalogState {
  refresh: (directory: boolean) => void;
  retry: () => void;
}
export const initialState: CatalogState = {
  processors: [],
  diagnostics: [],
  failure: null,
  busy: true,
};
