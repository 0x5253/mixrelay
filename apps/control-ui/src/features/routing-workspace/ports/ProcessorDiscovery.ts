import type { InventoryPort } from '../../../core/ports/control/InventoryPort';

export type CatalogAction = 'initial' | 'scan' | 'directory';
export type CatalogPort = Pick<
  InventoryPort,
  'getProcessors' | 'rescanProcessors' | 'selectProcessorDirectory'
>;
