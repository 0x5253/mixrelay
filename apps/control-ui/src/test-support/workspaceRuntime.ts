import { monotonicClock, timerScheduler } from '../core/adapters/browser/timers';
import type { AudioGraphModel } from '../core/audio-graph/model';
import type { WorkspaceRuntime } from '../core/ports/WorkspaceRuntime';
import { audioGraphToRuntimeGraph } from '../features/routing-workspace/model';

export const workspaceRuntime: WorkspaceRuntime<AudioGraphModel> = {
  scheduler: timerScheduler,
  clock: monotonicClock,
  operationId: () => crypto.randomUUID(),
  toDocument: audioGraphToRuntimeGraph,
};
