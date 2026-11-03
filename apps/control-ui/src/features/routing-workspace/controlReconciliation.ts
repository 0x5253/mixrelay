import type { AudioGraphModel } from '../../core/audio-graph/model';
import type { ControlChange } from '../../core/domain/graph/controlChanges';

type GraphNode = AudioGraphModel['nodes'][number];

/** Preserve every layout/topology edit while replacing only externally changed control fields.
 * Pass one `rebased` cache to every snapshot of a history so a node they share stays shared;
 * a snapshot with no affected node is returned unchanged. */
export function rebaseControls(
  model: AudioGraphModel,
  changes: readonly ControlChange[],
  rebased = new Map<GraphNode, GraphNode>(),
): AudioGraphModel {
  const byId = new Map(changes.map((change) => [change.id, change]));
  let changed = false;
  const nodes = model.nodes.map((node) => {
    let next = rebased.get(node);
    if (!next) {
      next = rebaseNode(node, byId);
      rebased.set(node, next);
    }
    changed ||= next !== node;
    return next;
  });
  return changed ? { ...model, nodes } : model;
}

function rebaseNode(node: GraphNode, byId: ReadonlyMap<string, ControlChange>): GraphNode {
  const runtime = node.runtime;
  const change = byId.get(runtime?.binding?.nodeId ?? node.id);
  if (!runtime || !change || runtime.type !== change.type) return node;
  const { gainDb, ...control } = change.value;
  const appearance = { ...node.appearance };
  let config = runtime.config;
  if (gainDb !== undefined && config && typeof config === 'object') {
    config = { ...config, gainDb };
    if (appearance.gainControl)
      appearance.gainControl = { ...appearance.gainControl, valueDb: gainDb };
    if (runtime.type === 'gain') appearance.detail = `${gainDb.toFixed(1)} dB`;
  }
  return {
    ...node,
    appearance,
    runtime: {
      ...runtime,
      config,
      control: {
        enabled: true,
        muted: false,
        ...runtime.control,
        ...control,
      },
    },
  };
}

/** Rebases every history snapshot and the live model with one node cache, so a node they
 * share stays one object. */
export function rebaseControlsWithHistory(
  model: AudioGraphModel,
  changes: readonly ControlChange[],
  rebaseHistory: (transform: (snapshot: AudioGraphModel) => AudioGraphModel) => void,
): AudioGraphModel {
  const rebased = new Map<GraphNode, GraphNode>();
  rebaseHistory((snapshot) => rebaseControls(snapshot, changes, rebased));
  return rebaseControls(model, changes, rebased);
}
