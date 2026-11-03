import type { RuntimeGraphDocument } from '../control/graph';

/** A session acknowledgement cannot replace a newer catalog document. */
export function workspaceSessionDocument(
  sessionDocument: RuntimeGraphDocument | undefined,
  catalogRevision: number | undefined,
  fallback: RuntimeGraphDocument | undefined,
) {
  return sessionDocument && sessionDocument.revision >= (catalogRevision ?? 0)
    ? sessionDocument
    : fallback;
}
