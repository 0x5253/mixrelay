/** Returns `next` with every subtree that deep-equals the matching part of `previous`
 * replaced by the previous object, so unchanged data keeps its identity across polls and
 * presentation passes and memoized consumers skip work. Plain objects, arrays and Map
 * entries are compared by value; every other value (functions, class instances) by identity. */
export function structuralShare<T>(previous: unknown, next: T): T {
  if (Object.is(previous, next)) return next;
  if (Array.isArray(next))
    return (Array.isArray(previous) ? shareArray(previous, next) : next) as T;
  if (next instanceof Map) return (previous instanceof Map ? shareMap(previous, next) : next) as T;
  if (!isPlainObject(next) || !isPlainObject(previous)) return next;
  return shareObject(previous, next) as T;
}

function shareArray(previous: unknown[], next: unknown[]) {
  // Index by index, holes included: `every`/`map` would skip holes and hide a change.
  let same = previous.length === next.length;
  const shared: unknown[] = new Array(next.length);
  for (let index = 0; index < next.length; index += 1) {
    const present = index in next;
    if (present) shared[index] = structuralShare(previous[index], next[index]);
    if (
      same &&
      (present !== index in previous || (present && !Object.is(shared[index], previous[index])))
    )
      same = false;
  }
  return same ? previous : shared;
}

function shareObject(previous: Record<string, unknown>, next: Record<string, unknown>) {
  const keys = Object.keys(next);
  let same = keys.length === Object.keys(previous).length;
  const shared: Record<string, unknown> = {};
  for (const key of keys) {
    const value = structuralShare(previous[key], next[key]);
    shared[key] = value;
    if (same && (!Object.is(value, previous[key]) || !(key in previous))) same = false;
  }
  return same ? previous : shared;
}

function shareMap(previous: Map<unknown, unknown>, next: Map<unknown, unknown>) {
  let same = previous.size === next.size;
  const shared = new Map();
  for (const [key, value] of next) {
    const kept = structuralShare(previous.get(key), value);
    shared.set(key, kept);
    if (same && (!Object.is(kept, previous.get(key)) || !previous.has(key))) same = false;
  }
  return same ? previous : shared;
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  if (typeof value !== 'object' || value === null) return false;
  const prototype = Object.getPrototypeOf(value);
  return prototype === Object.prototype || prototype === null;
}
