import type { GraphPort } from './GraphPort';
import type { InventoryPort } from './InventoryPort';
import type { ProcessorEditorPort } from './ProcessorEditorPort';
import type { RecorderPort } from './RecorderPort';
import type { TelemetryPort } from './TelemetryPort';
import type { WorkspacePort } from './WorkspacePort';

/** The engine control surface one adapter implements. Consumers that need one capability
 * depend on that capability port instead. */
export interface ControlPort
  extends GraphPort,
    TelemetryPort,
    RecorderPort,
    ProcessorEditorPort,
    InventoryPort,
    WorkspacePort {}
