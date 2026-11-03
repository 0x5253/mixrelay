import type { RuntimeGraphDocument } from '../control/graph';

function same(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (Array.isArray(a) && Array.isArray(b))
    return a.length === b.length && a.every((value, index) => same(value, b[index]));
  if (record(a) && record(b)) {
    const keys = Object.keys(a);
    return keys.length === Object.keys(b).length && keys.every((key) => same(a[key], b[key]));
  }
  return false;
}
function record(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}
function edited(before: unknown, local: unknown, current: unknown): unknown {
  if (same(before, local)) return current;
  if (!record(before) || !record(local) || !record(current)) return local;
  return Object.fromEntries(
    [...new Set([...Object.keys(before), ...Object.keys(local), ...Object.keys(current)])]
      .map((key) => [key, edited(before[key], local[key], current[key])])
      .filter(([, value]) => value !== undefined),
  );
}
function collection<T extends { id: string }>(before: T[], local: T[], current: T[]): T[] {
  const old = new Map(before.map((entry) => [entry.id, entry]));
  const draft = new Map(local.map((entry) => [entry.id, entry]));
  const present = new Set(current.map((entry) => entry.id));
  return [
    ...current.flatMap((entry) => {
      const base = old.get(entry.id);
      const change = draft.get(entry.id);
      if (!base) return [entry]; // A concurrent add owns its identity.
      if (replaced(base, entry)) return [entry];
      if (!change) return []; // Remove only the identity the user actually saw.
      return [edited(base, change, entry) as T];
    }),
    ...local.filter((entry) => !old.has(entry.id) && !present.has(entry.id)),
  ];
}

function replaced(before: unknown, current: unknown) {
  if (!record(before) || !record(current) || !('type' in before)) return false;
  if (before.type !== current.type) return true;
  const old = before.config;
  const next = current.config;
  if (!record(old) || !record(next)) return false;
  if (old.typeId !== next.typeId) return true;
  return record(old.effect) && record(next.effect) && old.effect.kind !== next.effect.kind;
}

/** Apply only this user's changed fields to a fresh acknowledged document.
 * Concurrent additions/values survive; removed or replaced nodes are never resurrected.
 * This is used only before a write or after a definite CAS rejection, never after a lost acknowledgement.
 */
export function rebaseGraphEdit(
  before: RuntimeGraphDocument,
  local: RuntimeGraphDocument,
  current: RuntimeGraphDocument,
): RuntimeGraphDocument {
  const nodes = collection(before.nodes, local.nodes, current.nodes);
  const ids = new Set(nodes.map((node) => node.id));
  const edges = collection(before.edges, local.edges, current.edges).filter(
    (edge) => ids.has(edge.source.nodeId) && ids.has(edge.destination.nodeId),
  );
  return { ...current, nodes, edges };
}
