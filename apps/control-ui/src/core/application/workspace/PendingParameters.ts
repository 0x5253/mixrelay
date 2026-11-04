import type { ProcessorParameterChange } from '../../domain/control/graph';

/** Processor parameter edits waiting for the next graph write, newest per parameter. */
export class PendingParameters {
  private changes = new Map<string, ProcessorParameterChange>();

  add(changes: readonly ProcessorParameterChange[]) {
    for (const change of changes)
      this.changes.set(JSON.stringify([change.nodeId, change.typeId, change.parameterId]), change);
  }

  /** Hands the current batch to one write; edits made meanwhile start a new batch. */
  take() {
    const batch = this.changes;
    this.changes = new Map();
    return batch;
  }

  /** Returns an unwritten batch; newer edits to the same parameter win. */
  restore(batch: ReadonlyMap<string, ProcessorParameterChange>) {
    this.changes = new Map([...batch, ...this.changes]);
  }

  clear() {
    this.changes.clear();
  }
}
