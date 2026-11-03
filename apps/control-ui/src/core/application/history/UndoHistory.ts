import type { Cancel, Clock, Scheduler } from '../../ports/Scheduler';

/** Older entries are dropped beyond this many. */
export const HISTORY_LIMIT = 100;
/** Recorded edits of one group and kind closer together than this become one undo step. */
export const MERGE_WINDOW_MS = 600;
/** A live gesture becomes one undo step once its control stays idle this long. */
export const GESTURE_IDLE_MS = 400;

interface HistoryEntry<Value, Kind> {
  after: Value;
  before: Value;
  changedAt: number;
  group?: string;
  kind: Kind;
}

export interface HistoryStep<Value, Kind> {
  kind: Kind;
  value: Value;
}

/** Undo and redo stacks of whole snapshots. Live gestures (a dragged fader) collect into
 * one pending entry that commits when the control goes idle or another edit arrives. */
export class UndoHistory<Value, Kind> {
  private undoStack: HistoryEntry<Value, Kind>[] = [];
  private redoStack: HistoryEntry<Value, Kind>[] = [];
  private navigatedSinceRecord = false;
  private pending: HistoryEntry<Value, Kind> | null = null;
  private cancelPending?: Cancel;

  constructor(
    private readonly clock: Clock,
    private readonly scheduler: Scheduler,
  ) {}

  /** Commits the pending gesture now. */
  flush() {
    this.cancelTimer();
    const pending = this.pending;
    this.pending = null;
    if (pending) this.commit(pending);
  }

  clear() {
    this.cancelTimer();
    this.pending = null;
    this.undoStack = [];
    this.redoStack = [];
    this.navigatedSinceRecord = false;
  }

  /** Drops a scheduled commit without committing, for an owner that goes away. */
  dispose() {
    this.cancelTimer();
  }

  record(before: Value, after: Value, kind: Kind, group?: string) {
    // A structural edit must follow (not precede) the live control gesture awaiting commit.
    this.flush();
    const changedAt = this.clock.now();
    const previous = this.undoStack.at(-1);
    const merge =
      !this.navigatedSinceRecord &&
      group !== undefined &&
      previous?.group === group &&
      previous.kind === kind &&
      changedAt - previous.changedAt <= MERGE_WINDOW_MS;
    // Merging needs a record since the last undo/redo, so there is nothing to redo.
    if (merge && previous) {
      previous.after = after;
      previous.changedAt = changedAt;
    } else {
      this.commit({ after, before, changedAt, group, kind });
    }
  }

  recordDebounced(
    before: Value,
    after: Value,
    kind: Kind,
    group: string,
    delayMs = GESTURE_IDLE_MS,
  ) {
    const previous = this.pending;
    if (previous && (previous.group !== group || previous.kind !== kind)) this.flush();
    const changedAt = this.clock.now();
    this.pending = this.pending
      ? { ...this.pending, after, changedAt, kind }
      : { after, before, changedAt, group, kind };
    this.cancelTimer();
    this.cancelPending = this.scheduler.after(delayMs, () => this.flush());
  }

  undo(): HistoryStep<Value, Kind> | null {
    this.flush();
    const entry = this.undoStack.pop();
    if (!entry) return null;
    this.redoStack.push(entry);
    this.navigatedSinceRecord = true;
    return { kind: entry.kind, value: entry.before };
  }

  redo(): HistoryStep<Value, Kind> | null {
    this.flush();
    const entry = this.redoStack.pop();
    if (!entry) return null;
    this.undoStack.push(entry);
    this.navigatedSinceRecord = true;
    return { kind: entry.kind, value: entry.after };
  }

  /** Adjacent entries share snapshots (one's `after` is the next one's `before`). Each
   * distinct snapshot is transformed once, so the rebased history keeps sharing them. */
  rebase(transform: (value: Value) => Value) {
    const entries = [...this.undoStack, ...this.redoStack];
    if (this.pending) entries.push(this.pending);
    const rebased = new Map<Value, Value>();
    const once = (value: Value) => {
      if (!rebased.has(value)) rebased.set(value, transform(value));
      // Present: set on the line above when it was missing.
      return rebased.get(value) as Value;
    };
    for (const entry of entries) {
      entry.before = once(entry.before);
      entry.after = once(entry.after);
    }
  }

  private commit(entry: HistoryEntry<Value, Kind>) {
    this.undoStack.push(entry);
    if (this.undoStack.length > HISTORY_LIMIT) this.undoStack.shift();
    this.redoStack = [];
    this.navigatedSinceRecord = false;
  }

  private cancelTimer() {
    this.cancelPending?.();
    this.cancelPending = undefined;
  }
}
