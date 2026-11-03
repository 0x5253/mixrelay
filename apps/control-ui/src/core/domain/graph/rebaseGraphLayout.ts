import type { GraphLayout } from '../control/catalog';

function fields<T extends object>(base: T | undefined, local: T, remote: T): T {
  return Object.fromEntries(
    Object.entries(local).map(([key, value]) => [
      key,
      value === base?.[key as keyof T] ? remote[key as keyof T] : value,
    ]),
  ) as T;
}

function entries<T extends object>(
  base: Record<string, T>,
  local: Record<string, T>,
  remote: Record<string, T>,
): Record<string, T> {
  const result = { ...remote };
  for (const [id, value] of Object.entries(local)) {
    const previous = base[id];
    const current = remote[id];
    // Remote removal and concurrent additions own their identities.
    if (previous && !current) continue;
    if (!previous && current) continue;
    result[id] = current ? fields(previous, value, current) : value;
  }
  for (const id of Object.keys(base)) {
    if (!local[id]) delete result[id];
  }
  return result;
}

/** Apply changed geometry fields only; a removed remote entry cannot be revived. */
export function rebaseGraphLayout(
  base: GraphLayout,
  local: GraphLayout,
  remote: GraphLayout,
  nodeIds?: ReadonlySet<string>,
): GraphLayout {
  const positions = entries(base.positions, local.positions, remote.positions);
  const sizes = entries(base.sizes ?? {}, local.sizes ?? {}, remote.sizes ?? {});
  const nodeLabels = labels(base.nodeLabels ?? {}, local.nodeLabels ?? {}, remote.nodeLabels ?? {});
  removeDeletedGeometry(base, remote, positions, sizes);
  prune(nodeLabels, nodeIds);
  prune(positions, nodeIds);
  prune(sizes, nodeIds);
  const viewport = mergeViewport(base.viewport, local.viewport, remote.viewport);
  return {
    positions,
    ...(Object.keys(nodeLabels).length ? { nodeLabels } : {}),
    ...(Object.keys(sizes).length ? { sizes } : {}),
    ...(viewport ? { viewport } : {}),
  };
}

function removeDeletedGeometry(
  base: GraphLayout,
  remote: GraphLayout,
  positions: GraphLayout['positions'],
  sizes: NonNullable<GraphLayout['sizes']>,
) {
  for (const id of new Set([...Object.keys(base.positions), ...Object.keys(base.sizes ?? {})])) {
    if (!remote.positions[id] && !remote.sizes?.[id]) {
      delete positions[id];
      delete sizes[id];
    }
  }
}

function prune(entries: Record<string, unknown>, nodeIds?: ReadonlySet<string>) {
  if (!nodeIds) return;
  for (const id of Object.keys(entries)) if (!nodeIds.has(id)) delete entries[id];
}

function mergeViewport(
  base: GraphLayout['viewport'],
  local: GraphLayout['viewport'],
  remote: GraphLayout['viewport'],
) {
  if (!local) return base ? undefined : remote;
  if (!remote) {
    const unchanged = base && local.x === base.x && local.y === base.y && local.zoom === base.zoom;
    return unchanged ? undefined : local;
  }
  return fields(base, local, remote);
}

/** A label reset removes an override, not the node; changed local labels still merge by UI ID. */
function labels(
  base: Record<string, string>,
  local: Record<string, string>,
  remote: Record<string, string>,
) {
  const result = new Map(Object.entries(remote));
  const before = new Map(Object.entries(base));
  const after = new Map(Object.entries(local));
  for (const id of new Set([...Object.keys(base), ...Object.keys(local)])) {
    const value = after.get(id);
    if (value === before.get(id)) continue;
    if (value) result.set(id, value);
    else result.delete(id);
  }
  return Object.fromEntries(result);
}
