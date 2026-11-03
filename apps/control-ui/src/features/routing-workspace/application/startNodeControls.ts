import type { GraphNodeControls } from '../../../core/domain/control/graph';
import { graphControls } from '../../../core/domain/control/state';
import type { GraphPort } from '../../../core/ports/control/GraphPort';
import type { TelemetryPort } from '../../../core/ports/control/TelemetryPort';
import type { Cancel, Scheduler } from '../../../core/ports/Scheduler';

type NodeControlsPort = Pick<GraphPort, 'getNodeControls'> & Pick<TelemetryPort, 'subscribeState'>;
interface NodeReadbackView {
  current(): { revision: number; idle: boolean };
  refresh(): void;
  publish(snapshot: GraphNodeControls | null): void;
}

export function startNodeControls(
  port: NodeControlsPort,
  graphId: string,
  scheduler: Scheduler,
  view: NodeReadbackView,
) {
  let cancelled = false;
  let timer: Cancel;
  let incarnation: string | undefined;
  let resync = false;
  const canRead = () => !cancelled && view.current().idle;
  const receive = (result: GraphNodeControls | undefined) => {
    if (!canRead()) return;
    if (resync) {
      resync = false;
      view.refresh();
    }
    if (result?.graphId !== graphId) return;
    view.publish(result);
    if (result.revision > view.current().revision) view.refresh();
  };
  let pushed = false;
  let committed: GraphNodeControls | undefined;
  let generation = 0;
  const unsubscribe = port.subscribeState?.((state, engine) => {
    if (state && engine) {
      resync ||= incarnation !== undefined && incarnation !== engine;
      incarnation = engine;
    }
    generation += 1;
    pushed = Boolean(state);
    committed = graphControls(state, graphId);
    receive(committed);
    if (!committed) view.publish(null);
  });
  const schedule = () => {
    if (!cancelled) timer = scheduler.after(1000, () => void poll());
  };
  const ready = () => {
    if (pushed) receive(committed);
    return !pushed && canRead();
  };
  const poll = async () => {
    const attempt = generation;
    const current = () => !cancelled && attempt === generation;
    if (!ready()) {
      schedule();
      return;
    }
    try {
      const result = await port.getNodeControls(graphId);
      if (current()) receive(result);
    } catch {
      // An unreachable controller must not leave apparently actionable stale buttons.
      if (current()) view.publish(null);
    } finally {
      schedule();
    }
  };
  void poll();
  return () => {
    cancelled = true;
    unsubscribe?.();
    timer?.();
  };
}
