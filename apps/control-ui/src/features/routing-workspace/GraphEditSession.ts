import type { HistoryStep } from '../../core/application/history/UndoHistory';
import type { GraphApplicationQueue } from '../../core/application/workspace/GraphApplicationQueue';
import type { AudioGraphChangeKind } from '../../core/audio-graph/audioGraphCanvasTypes';
import type { AudioGraphModel } from '../../core/audio-graph/model';
import { graphHistoryGroup } from './graphHistoryGroup';
import type { PendingProcessorParameter } from './ProcessorParameterQueue';
import { changedProcessorParameters } from './processorParameterChanges';

export type GraphHistoryStep = HistoryStep<AudioGraphModel, AudioGraphChangeKind>;

export interface GraphEditHistory {
  record(
    before: AudioGraphModel,
    after: AudioGraphModel,
    kind: AudioGraphChangeKind,
    group?: string,
  ): void;
  recordDebounced(
    before: AudioGraphModel,
    after: AudioGraphModel,
    kind: AudioGraphChangeKind,
    group: string,
  ): void;
  flush(): void;
  undo(): GraphHistoryStep | null;
  redo(): GraphHistoryStep | null;
}

/** Where an accepted edit goes: the view, saved node positions, and the engine. */
export interface GraphEditTargets {
  history: GraphEditHistory;
  queue: Pick<GraphApplicationQueue<AudioGraphModel>, 'canPublishParameters' | 'submit'>;
  publishParameter(update: PendingProcessorParameter): void;
  /** The loaded document's revision, which processor parameter commands expect. */
  graphRevision(): number;
  show(model: AudioGraphModel): void;
  savePositions(model: AudioGraphModel): void;
}

/** Level, parameter and band gestures coalesce into one Undo step while they continue. */
function coalescesGesture(kind: AudioGraphChangeKind) {
  return kind === 'level' || kind === 'processor_parameter' || kind === 'parameter_group';
}

/** An Undo step restores a snapshot as a new revision; layout carries no revision. */
function restoredModel(step: GraphHistoryStep, current: AudioGraphModel): AudioGraphModel {
  return {
    ...step.value,
    revision:
      step.kind === 'layout'
        ? current.revision
        : Math.max(current.revision, step.value.revision) + 1,
  };
}

/** The canvas edits of one workspace owner (a control port and graph). A callback retained
 * past its owner, by a replaced port or graph or an unmounted workspace, changes nothing,
 * and a new owner edits only once its view has mounted. Callbacks act through the targets
 * of the latest render. */
export class GraphEditSession {
  private owner: object | null = null;
  /** Owners whose view is mounted; a discarded render cannot close the committed one. */
  private readonly opened = new WeakSet<object>();
  private targets!: GraphEditTargets;

  /** `model` is shared with the application queue, which rebases it after readback. */
  constructor(private readonly model: { current: AudioGraphModel | null }) {}

  /** Records the owner and targets of the latest render; it has no other effect. */
  render(owner: object, targets: GraphEditTargets) {
    this.owner = owner;
    this.targets = targets;
  }

  open(owner: object) {
    this.opened.add(owner);
  }

  close(owner: object) {
    this.opened.delete(owner);
  }

  /** Accepts an edit only from the current, mounted owner, which has a loaded model. */
  edit(owner: object, next: AudioGraphModel, kind: AudioGraphChangeKind) {
    if (owner !== this.owner || !this.opened.has(owner)) return;
    const previous = this.model.current as AudioGraphModel;
    if (next !== previous) this.record(previous, next, kind);
    this.apply(previous, next, kind);
  }

  undo() {
    return this.live && this.restore(this.targets.history.undo());
  }

  redo() {
    return this.live && this.restore(this.targets.history.redo());
  }

  /** Before its owner mounts, history may still hold the previous owner's steps. */
  private get live() {
    return this.owner !== null && this.opened.has(this.owner);
  }

  private record(before: AudioGraphModel, after: AudioGraphModel, kind: AudioGraphChangeKind) {
    const { history } = this.targets;
    const group = graphHistoryGroup(before, after, kind);
    if (coalescesGesture(kind) && group) {
      history.recordDebounced(before, after, kind, group);
      return;
    }
    history.flush();
    history.record(before, after, kind, group);
  }

  /** History exists only after a loaded owner recorded it, and is cleared with its owner. */
  private restore(step: GraphHistoryStep | null) {
    if (!step) return false;
    const previous = this.model.current as AudioGraphModel;
    this.apply(previous, restoredModel(step, previous), step.kind);
    return true;
  }

  private apply(previous: AudioGraphModel, next: AudioGraphModel, kind: AudioGraphChangeKind) {
    const { targets } = this;
    this.model.current = next;
    targets.show(next);
    targets.savePositions(next);
    if (kind === 'layout') return;
    const parameters =
      kind === 'processor_parameter' ? changedProcessorParameters(previous, next) : [];
    if (kind === 'processor_parameter' && targets.queue.canPublishParameters) {
      for (const update of parameters)
        targets.publishParameter({ ...update, expectedRevision: targets.graphRevision() });
      return;
    }
    targets.queue.submit(next, parameters);
  }
}
