import type { NodeControlRequest, RuntimeGraphDocument, RuntimeGraphNode } from '../control/graph';

export type ControlChange = {
  id: string;
  type: string;
  value: Omit<NodeControlRequest, 'expectedRevision'>;
};
const gainTypes = new Set([
  'gain',
  'virtual_endpoint',
  'audio_device_input',
  'application_capture',
]);

function ordered(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(ordered);
  if (value && typeof value === 'object')
    return Object.fromEntries(
      Object.entries(value)
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([key, item]) => [key, ordered(item)]),
    );
  return value;
}

function config(node: RuntimeGraphNode): Record<string, unknown> {
  return node.config && typeof node.config === 'object' ? { ...node.config } : {};
}

function changedGain(old: RuntimeGraphNode, node: RuntimeGraphNode) {
  const gain = config(node).gainDb ?? 0;
  return gainTypes.has(node.type) && gain !== (config(old).gainDb ?? 0) && typeof gain === 'number'
    ? gain
    : undefined;
}

function structural(document: RuntimeGraphDocument) {
  return ordered({
    ...document,
    revision: 0,
    nodes: document.nodes.map((node) => {
      const { control: _control, ...rest } = node;
      if (!gainTypes.has(node.type)) return rest;
      const value = config(node);
      delete value.gainDb;
      return { ...rest, config: value };
    }),
  });
}

/** Only an exact topology/config match permits external controls to rebase edit history. */
export function controlChanges(
  before: RuntimeGraphDocument,
  after: RuntimeGraphDocument,
): ControlChange[] | null {
  if (JSON.stringify(structural(before)) !== JSON.stringify(structural(after))) return null;
  const previous = new Map(before.nodes.map((node) => [node.id, node]));
  return after.nodes.flatMap((node) => {
    // The exact structural match above guarantees identical node IDs.
    const old = previous.get(node.id) as RuntimeGraphNode;
    const value: ControlChange['value'] = {};
    for (const key of ['enabled', 'muted'] as const) {
      const defaultValue = key === 'enabled';
      if ((old.control?.[key] ?? defaultValue) !== (node.control?.[key] ?? defaultValue))
        value[key] = node.control?.[key] ?? defaultValue;
    }
    const gain = changedGain(old, node);
    if (gain !== undefined) value.gainDb = gain;
    return Object.keys(value).length ? [{ id: node.id, type: node.type, value }] : [];
  });
}
