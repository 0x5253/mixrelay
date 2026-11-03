import { useCallback, useEffect, useRef, useState } from 'react';
import { timerScheduler } from '../../core/adapters/browser/timers';
import type { AudioGraphModel } from '../../core/audio-graph/model';
import type { ProcessorEditorStatus } from '../../core/domain/control/processors';
import { structuralShare } from '../../core/domain/structuralShare';
import type { ControlPort } from '../../core/ports/control/ControlPort';
import type { Scheduler } from '../../core/ports/Scheduler';
import { EditorSession, type ProcessorEditorIssue } from './application/EditorSession';
import { editorsToOpen } from './domain/editorHistory';

type EditorPort = Pick<ControlPort, 'getProcessorEditor' | 'setProcessorEditor' | 'subscribeState'>;

function identities(model: AudioGraphModel | null) {
  return (
    model?.nodes
      .filter(
        (node) =>
          node.runtime?.type === 'provider' &&
          node.runtime.control?.enabled !== false &&
          node.appearance.hasProcessorEditor,
      )
      .map((node) => {
        const config = node.runtime?.config as
          | { typeId?: string; ports?: { layoutId?: string } }
          | undefined;
        // A layout change replaces the native instance, so it is part of the identity.
        const identity = JSON.stringify([config?.typeId ?? '', config?.ports?.layoutId ?? '']);
        return [node.id, identity, node.appearance.title] as const;
      }) ?? []
  );
}

export function useProcessorEditors(
  controlPort: EditorPort,
  model: AudioGraphModel | null,
  graphId?: string,
  scheduler: Scheduler = timerScheduler,
) {
  const [statuses, setStatuses] = useState<Map<string, ProcessorEditorStatus>>(() => new Map());
  const [issues, setIssues] = useState<Map<string, ProcessorEditorIssue>>(() => new Map());
  const statusesRef = useRef(statuses);
  statusesRef.current = statuses;
  const sessionRef = useRef<EditorSession | null>(null);
  const reopenRef = useRef(new Map<string, number>());
  const previous = useRef<{
    port: EditorPort;
    keys: Map<string, string>;
    graphId?: string;
    revision: number;
  } | null>(null);
  const key = JSON.stringify([model?.revision ?? 0, identities(model)]);
  useEffect(() => {
    const [expectedRevision, nodes]: [number, [string, string, string][]] = JSON.parse(key);
    const keys = new Map(nodes.map(([id, type]) => [id, type]));
    const prior = previous.current;
    const before = statusesRef.current;
    const samePlace = prior?.port === controlPort && prior.graphId === graphId;
    // Intents belong to one graph on one port; node IDs repeat across graphs.
    if (!samePlace) reopenRef.current.clear();
    statusesRef.current = new Map(
      [...before].filter(
        ([id]) => samePlace && keys.has(id) && prior?.keys.get(id) === keys.get(id),
      ),
    );
    const autoOpen = editorsToOpen(nodes, {
      kept: statusesRef.current,
      before: samePlace ? before : new Map(),
      reopen: reopenRef.current,
      // Used only for replaced nodes, which exist only when a prior session does.
      priorRevision: prior?.revision ?? expectedRevision,
      revision: expectedRevision,
    });
    setStatuses(statusesRef.current);
    previous.current = { port: controlPort, keys, graphId, revision: expectedRevision };
    setIssues(new Map());
    const session = new EditorSession(
      controlPort,
      nodes,
      {
        removeStatus: (id) => {
          if (!statusesRef.current.has(id)) return;
          statusesRef.current = new Map(statusesRef.current);
          statusesRef.current.delete(id);
          setStatuses(statusesRef.current);
        },
        invalidate: () => {
          statusesRef.current = new Map();
          setStatuses(new Map());
        },
        status: (status) => {
          // Polled every few hundred ms: an unchanged status must not re-render the graph.
          const previous = statusesRef.current.get(status.nodeId);
          if (structuralShare(previous, status) === previous) return;
          statusesRef.current = new Map(statusesRef.current).set(status.nodeId, status);
          setStatuses(statusesRef.current);
        },
        issue: (entry, busy) =>
          setIssues((current) => {
            const existing = current.get(entry.id);
            // Every poll reports its issue state; an unchanged one must not re-render.
            // An entry's name is fixed for its lifetime; the session resets the map.
            if (busy === null ? !existing : existing?.busy === busy) return current;
            const next = new Map(current);
            if (busy === null) next.delete(entry.id);
            else next.set(entry.id, { nodeId: entry.id, name: entry.name, busy });
            return next;
          }),
        busy: (entry) =>
          setIssues((current) =>
            current.has(entry.id)
              ? new Map(current).set(entry.id, { nodeId: entry.id, name: entry.name, busy: true })
              : current,
          ),
      },
      expectedRevision,
      autoOpen,
      scheduler,
    );
    sessionRef.current = session;
    session.start();
    return () => {
      session.dispose();
      sessionRef.current = null;
    };
  }, [controlPort, key, graphId, scheduler]);
  const retry = useCallback((id: string) => sessionRef.current?.retry(id), []);
  return { statuses, issues, retry };
}
