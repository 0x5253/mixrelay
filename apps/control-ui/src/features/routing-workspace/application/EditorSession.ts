import type { ProcessorEditorStatus } from '../../../core/domain/control/processors';
import type { ControlPort } from '../../../core/ports/control/ControlPort';
import type { Cancel, Scheduler } from '../../../core/ports/Scheduler';

export const EDITOR_POLL_MS = 250;
type EditorPort = Pick<ControlPort, 'getProcessorEditor' | 'setProcessorEditor' | 'subscribeState'>;
type Action = { kind: 'poll' } | { kind: 'open' };
export interface ProcessorEditorIssue {
  nodeId: string;
  name: string;
  busy: boolean;
}
interface Entry {
  id: string;
  name: string;
  sequence: number;
  command: boolean;
  /** Present a new node once; a user-closed native window stays closed across graph edits. */
  autoOpen: boolean;
  retry: Action;
  timer?: Cancel;
}
interface Callbacks {
  removeStatus: (id: string) => void;
  invalidate: () => void;
  status: (status: ProcessorEditorStatus) => void;
  issue: (entry: Entry, busy: boolean | null) => void;
  busy: (entry: Entry) => void;
}
/** Node/port lifetime and per-node request ordering stay independent of React render timing. */
export class EditorSession {
  private cancelled = false;
  private pushed = false;
  private generation = 0;
  private latest?: ProcessorEditorStatus[];
  private unsubscribe?: () => void;
  private entries: Map<string, Entry>;
  constructor(
    private port: EditorPort,
    nodes: [string, string, string][],
    private callbacks: Callbacks,
    private expectedRevision: number,
    autoOpen: ReadonlySet<string>,
    private scheduler: Scheduler,
  ) {
    this.entries = new Map(
      nodes.map(([id, , name]) => [
        id,
        {
          id,
          name,
          sequence: 0,
          command: false,
          autoOpen: autoOpen.has(id),
          retry: { kind: 'poll' },
        },
      ]),
    );
  }
  start() {
    this.unsubscribe = this.port.subscribeState?.((state) => {
      this.generation += 1;
      this.pushed = Boolean(state);
      this.latest =
        state?.graph.submittedRevision === this.expectedRevision ? state.editors : undefined;
      if (!this.pushed) this.callbacks.invalidate();
      for (const entry of this.entries.values()) this.pushEntry(entry, this.latest);
    });
    for (const entry of this.entries.values()) void this.run(entry, { kind: 'poll' });
  }
  /** Applies pushed state, or without push forgets the stale status and polls instead. An
   * entry with a command in flight is applied when that command completes. */
  private pushEntry(entry: Entry, editors?: ProcessorEditorStatus[]) {
    if (entry.command) return;
    entry.sequence += 1;
    if (entry.timer) entry.timer?.();
    const status = this.pushed ? editors?.find((editor) => editor.nodeId === entry.id) : undefined;
    if (status) return this.receive(entry, status, { kind: 'poll' });
    this.callbacks.removeStatus(entry.id);
    this.callbacks.issue(entry, false);
    if (!this.pushed)
      entry.timer = this.scheduler.after(
        EDITOR_POLL_MS,
        () => void this.run(entry, { kind: 'poll' }),
      );
  }
  private valid(entry: Entry, sequence: number) {
    return !this.cancelled && this.entries.get(entry.id) === entry && sequence === entry.sequence;
  }
  private receive(entry: Entry, status: ProcessorEditorStatus, action: Action) {
    if (status.nodeId !== entry.id) throw new Error('Mismatched editor response');
    this.callbacks.status(status);
    if (
      status.state === 'failed' ||
      status.state === 'unavailable' ||
      (status.state === 'closed' && action.kind === 'open')
    ) {
      entry.retry = { kind: 'open' };
      this.callbacks.issue(entry, false);
      return;
    }
    this.callbacks.issue(entry, null);
    if (status.state === 'closed') {
      entry.retry = { kind: 'open' };
      if (entry.autoOpen) {
        entry.autoOpen = false;
        void this.run(entry, { kind: 'open' });
      }
      return;
    }
    // Failed, unavailable and closed states returned above; open/ opening keep polling.
    entry.autoOpen = false;
    if (!this.pushed)
      entry.timer = this.scheduler.after(EDITOR_POLL_MS, () => {
        void this.run(entry, { kind: 'poll' });
      });
  }
  private async run(entry: Entry, action: Action) {
    if (this.cancelled || entry.command || (action.kind === 'poll' && this.pushed)) return;
    if (entry.timer) entry.timer?.();
    const sequence = ++entry.sequence;
    const generation = this.generation;
    entry.command = action.kind === 'open';
    if (action.kind === 'open') entry.retry = action;
    this.callbacks.busy(entry);
    try {
      const status = await (action.kind === 'poll'
        ? this.port.getProcessorEditor(entry.id)
        : this.port.setProcessorEditor(entry.id, true, this.expectedRevision));
      if (!this.valid(entry, sequence)) return;
      entry.command = false;
      if (generation !== this.generation) this.pushEntry(entry, this.latest);
      else this.receive(entry, status, action);
    } catch {
      if (!this.valid(entry, sequence)) return;
      entry.command = false;
      if (generation !== this.generation) {
        this.pushEntry(entry, this.latest);
        return;
      }
      entry.retry = action;
      this.callbacks.issue(entry, false); // Keep the last confirmed native state.
    }
  }
  retry(id: string) {
    const entry = this.entries.get(id);
    if (entry) void this.run(entry, entry.retry);
  }
  dispose() {
    this.cancelled = true;
    this.unsubscribe?.();
    for (const entry of this.entries.values()) if (entry.timer) entry.timer?.();
    // Native editor ownership belongs to the processor runtime. The control API resolves
    // IDs in the selected graph, so a late close here could close a different graph's node.
  }
}
