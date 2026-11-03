import { expect, it, vi } from 'vitest';
import type { RuntimeGraphDocument } from '../../domain/control/graph';
import { workspaceSessionDocument } from '../../domain/graph/WorkspaceDocuments';
import { StudioSession } from './StudioSession';

const document: RuntimeGraphDocument = {
  schemaVersion: 2,
  revision: 3,
  nodes: [],
  edges: [],
};

it('keeps late acknowledgements in the runtime and catalog selection that issued them', () => {
  const owner = {};
  const first = new StudioSession(owner, document);
  const replacement = new StudioSession(owner, { ...document });
  const changed = vi.fn();
  const unsubscribe = first.subscribe(changed);
  const replacementChanged = vi.fn();
  replacement.subscribe(replacementChanged);
  unsubscribe();
  first.accept({ ...document, revision: 4 });
  expect(first.snapshot()?.revision).toBe(4);
  expect(replacement.snapshot()).toEqual(document);
  expect(changed).not.toHaveBeenCalled();
  expect(replacementChanged).not.toHaveBeenCalled();

  replacement.accept({ ...document, revision: 5 });
  expect(replacementChanged).toHaveBeenCalledOnce();
  expect(first.owner).toBe(owner);
});

it('uses a session acknowledgement only once it reaches the catalog document revision', () => {
  const latest = { ...document, revision: 5 };
  expect(workspaceSessionDocument(document, 5, latest)).toBe(latest);
  expect(workspaceSessionDocument(latest, 5, document)).toBe(latest);
  expect(workspaceSessionDocument(undefined, 5, document)).toBe(document);
  expect(workspaceSessionDocument(document, undefined, undefined)).toBe(document);
  expect(new StudioSession({}).snapshot()).toBeUndefined();
});
