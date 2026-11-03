import type { RuntimeGraphDocument } from '../domain/control/graph';
import type { MonotonicClock, Scheduler } from './Scheduler';

/** Runtime effects and document projection consumed by the workspace writer. */
export interface WorkspaceRuntime<Model> {
  scheduler: Scheduler;
  clock: MonotonicClock;
  operationId(): string;
  toDocument(model: Model, source: RuntimeGraphDocument): RuntimeGraphDocument;
}

export interface LayoutNodeProjection {
  nodeIds(document: RuntimeGraphDocument): ReadonlySet<string>;
}
