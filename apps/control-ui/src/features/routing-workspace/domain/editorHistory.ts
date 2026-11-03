import type { ProcessorEditorStatus } from '../../../core/domain/control/processors';

/** New nodes present their editor once. A node whose native instance is replaced (e.g. a
 * new channel layout) reopens only if its editor was open. The layout reaches the model
 * before the engine applies it, so until a newer document revision arrives the old, still
 * open instance may answer; the intent is kept (`reopen`, by revision of the change) and
 * spent on the first session at a later revision, which polls the replacement. */
export function editorsToOpen(
  nodes: readonly (readonly [string, string, string])[],
  history: {
    kept: ReadonlyMap<string, ProcessorEditorStatus>;
    before: ReadonlyMap<string, ProcessorEditorStatus>;
    reopen: Map<string, number>;
    /** The previous session's document revision: a change seen without a newer one is
     * still optimistic. */
    priorRevision: number;
    revision: number;
  },
) {
  const { kept, before, reopen, priorRevision, revision } = history;
  const present = new Set(nodes.map(([id]) => id));
  for (const id of reopen.keys()) if (!present.has(id)) reopen.delete(id);
  const open = new Set<string>();
  for (const [id] of nodes) {
    const replaced = kept.has(id) ? undefined : before.get(id);
    if (replaced?.state === 'open') reopen.set(id, priorRevision);
    const changedAt = reopen.get(id);
    if (changedAt !== undefined && revision > changedAt) {
      reopen.delete(id);
      open.add(id);
    } else if (!kept.has(id) && !replaced) open.add(id);
  }
  return open;
}
