import { act, cleanup, renderHook } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { AudioGraphModel } from '../../core/audio-graph/model';
import type { ProcessorEditorStatus } from '../../core/domain/control/processors';
import { controlState, statePush } from '../../test-support/controlTelemetry';
import { EDITOR_POLL_MS } from './application/EditorSession';
import { useProcessorEditors } from './useProcessorEditors';

function status(state: ProcessorEditorStatus['state']): ProcessorEditorStatus {
  return {
    protocolVersion: 1,
    nodeId: 'effect',
    state,
    width: null,
    height: null,
    resizable: false,
    failure: null,
  };
}
function model(type = 'vst3:one'): AudioGraphModel {
  return {
    revision: 1,
    edges: [],
    nodes: [
      {
        id: 'effect',
        inputPorts: [],
        outputPorts: [],
        position: { x: 0, y: 0 },
        appearance: {
          title: 'Effect',
          category: 'Processor',
          icon: 'effect',
          hasProcessorEditor: true,
        },
        runtime: { type: 'provider', config: { typeId: type } },
      },
    ],
  };
}
function port() {
  return {
    getProcessorEditor: vi
      .fn<(id: string) => Promise<ProcessorEditorStatus>>()
      .mockResolvedValue(status('closed')),
    setProcessorEditor: vi
      .fn<(id: string, open: boolean) => Promise<ProcessorEditorStatus>>()
      .mockResolvedValue(status('open')),
  };
}
async function settle() {
  await act(async () => {});
}
afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it('consumes editor replacements without polling, clears removed editors and resumes fallback on loss', async () => {
  vi.useFakeTimers();
  const initial = controlState(1, 1);
  initial.editors = [status('open')];
  const push = statePush(initial);
  const source = { ...port(), ...push };
  const view = renderHook(() => useProcessorEditors(source, model()));
  await settle();
  expect(view.result.current.statuses.get('effect')?.state).toBe('open');
  await act(() => vi.advanceTimersByTimeAsync(1000));
  expect(source.getProcessorEditor).not.toHaveBeenCalled();
  const replacement = controlState(2, 1);
  act(() => push.push(replacement));
  expect(view.result.current.statuses.size).toBe(0);
  act(() => push.push(replacement));
  expect(view.result.current.statuses.size).toBe(0);
  const otherRevision = controlState(3, 2);
  otherRevision.editors = [status('open')];
  act(() => push.push(otherRevision));
  await act(() => vi.advanceTimersByTimeAsync(1000));
  expect(source.getProcessorEditor).not.toHaveBeenCalled();
  act(() => push.push());
  expect(view.result.current.statuses.size).toBe(0);
  await act(() => vi.advanceTimersByTimeAsync(EDITOR_POLL_MS));
  expect(source.getProcessorEditor).toHaveBeenCalledOnce();
  view.unmount();
  expect(vi.getTimerCount()).toBe(0);
});

it.each([true, false])(
  'fences a pending native editor command after push is lost (success: %s)',
  async (success) => {
    vi.useFakeTimers();
    const initial = controlState(1, 1);
    initial.editors = [status('closed')];
    const push = statePush(initial);
    const source = { ...port(), ...push };
    const command = Promise.withResolvers<ProcessorEditorStatus>();
    source.setProcessorEditor.mockReturnValueOnce(command.promise);
    const view = renderHook(() => useProcessorEditors(source, model()));
    await settle();
    expect(source.setProcessorEditor).toHaveBeenCalledOnce();
    act(() => push.push());
    await act(async () => {
      if (success) command.resolve(status('open'));
      else command.reject(new Error('old engine'));
    });
    expect(view.result.current.statuses.size).toBe(0);
    expect(view.result.current.issues.get('effect')?.busy).toBe(false);
    // Without push, the fallback poll resumes once the command completed.
    source.getProcessorEditor.mockResolvedValueOnce(status('open'));
    await act(() => vi.advanceTimersByTimeAsync(EDITOR_POLL_MS));
    expect(source.getProcessorEditor).toHaveBeenCalledOnce();
    expect(view.result.current.statuses.get('effect')?.state).toBe('open');
    view.unmount();
  },
);

it('uses the newest pushed editor state after a pending open, ignoring a late poll rejection', async () => {
  vi.useFakeTimers();
  const push = statePush();
  const source = { ...port(), ...push };
  const poll = Promise.withResolvers<ProcessorEditorStatus>();
  source.getProcessorEditor.mockReturnValueOnce(poll.promise);
  const view = renderHook(() => useProcessorEditors(source, model()));
  const state = controlState(1, 1);
  state.editors = [status('closed')];
  const command = Promise.withResolvers<ProcessorEditorStatus>();
  source.setProcessorEditor.mockReturnValueOnce(command.promise);
  act(() => push.push(state));
  await act(async () => poll.reject(new Error('late read')));
  state.editors = [status('failed')];
  act(() => push.push({ ...state, generation: 2 }));
  await act(async () => command.resolve(status('open')));
  expect(view.result.current.statuses.get('effect')?.state).toBe('failed');
  view.unmount();
});

it('does not reopen an editor from a late response after the node is powered off', async () => {
  const source = port();
  const pending = Promise.withResolvers<ProcessorEditorStatus>();
  source.getProcessorEditor.mockReturnValueOnce(pending.promise);
  const initial = model();
  const view = renderHook(({ graph }) => useProcessorEditors(source, graph), {
    initialProps: { graph: initial },
  });
  const off = {
    ...initial,
    revision: 2,
    nodes: initial.nodes.map((node) => ({
      ...node,
      runtime: {
        type: 'provider',
        config: { typeId: 'vst3:one' },
        control: { enabled: false, muted: false },
      },
    })),
  };
  view.rerender({ graph: off });
  await act(async () => pending.resolve(status('closed')));
  expect(source.setProcessorEditor).not.toHaveBeenCalled();
  expect(view.result.current.statuses.size).toBe(0);
});

describe('processor editor error and request lifecycle', () => {
  it('reports status request failure and explicitly retries the read', async () => {
    const source = port();
    source.getProcessorEditor.mockRejectedValueOnce(new Error('offline'));
    const { result } = renderHook(() => useProcessorEditors(source, model()));
    await settle();
    expect(result.current.issues.get('effect')?.name).toBe('Effect');
    expect(result.current.statuses.has('effect')).toBe(false);
    act(() => result.current.retry('effect'));
    await settle();
    expect(source.getProcessorEditor).toHaveBeenCalledTimes(2);
    expect(source.setProcessorEditor).toHaveBeenCalledWith('effect', true, 1);
    expect(result.current.statuses.get('effect')?.state).toBe('open');
    expect(result.current.issues.size).toBe(0);
  });

  it('opens a closed editor automatically and explicitly retries a failed open', async () => {
    const source = port();
    source.setProcessorEditor
      .mockRejectedValueOnce(new Error('open denied'))
      .mockResolvedValue(status('open'));
    const { result } = renderHook(() => useProcessorEditors(source, model()));
    await settle();
    expect(result.current.statuses.get('effect')?.state).toBe('closed');
    expect(result.current.issues.has('effect')).toBe(true);
    act(() => result.current.retry('effect'));
    await settle();
    expect(source.setProcessorEditor.mock.calls).toEqual([
      ['effect', true, 1],
      ['effect', true, 1],
    ]);
    expect(result.current.statuses.get('effect')?.state).toBe('open');
    expect(result.current.issues.size).toBe(0);
  });

  it('does not optimistically open or send duplicate open commands', async () => {
    const source = port();
    const command = Promise.withResolvers<ProcessorEditorStatus>();
    source.setProcessorEditor.mockReturnValueOnce(command.promise);
    const { result } = renderHook(() => useProcessorEditors(source, model()));
    await settle();
    act(() => {
      result.current.retry('effect');
      result.current.retry('effect');
    });
    await settle();
    expect(source.setProcessorEditor).toHaveBeenCalledTimes(1);
    expect(result.current.statuses.get('effect')?.state).toBe('closed');
    await act(async () => command.resolve(status('open')));
    expect(result.current.statuses.get('effect')?.state).toBe('open');
  });
});

describe('processor editor stale responses and native failures', () => {
  it('does not resurrect a removed or replaced node from late command responses', async () => {
    const source = port();
    const pending = Promise.withResolvers<ProcessorEditorStatus>();
    source.setProcessorEditor.mockReturnValueOnce(pending.promise);
    const { result, rerender } = renderHook(({ graph }) => useProcessorEditors(source, graph), {
      initialProps: { graph: model() },
    });
    await settle();
    rerender({ graph: { revision: 2, edges: [], nodes: [] } });
    await settle();
    await act(async () => pending.resolve(status('open')));
    expect(result.current.statuses.size).toBe(0);
    expect(result.current.issues.size).toBe(0);
    rerender({ graph: model('vst3:replacement') });
    await settle();
    expect(result.current.statuses.get('effect')?.state).toBe('open');
  });

  it('ignores old-port failures and stops polling after unmount', async () => {
    vi.useFakeTimers();
    const first = port(),
      second = port();
    const pending = Promise.withResolvers<ProcessorEditorStatus>();
    first.getProcessorEditor.mockReturnValue(pending.promise);
    const { result, rerender, unmount } = renderHook(
      ({ source }) => useProcessorEditors(source, model()),
      { initialProps: { source: first } },
    );
    await settle();
    rerender({ source: second });
    await settle();
    await act(async () => pending.reject(new Error('old transport')));
    expect(result.current.issues.size).toBe(0);
    expect(result.current.statuses.get('effect')?.state).toBe('open');
    unmount();
    await act(async () => vi.advanceTimersByTimeAsync(EDITOR_POLL_MS * 4));
    expect(second.getProcessorEditor).toHaveBeenCalledTimes(1);
  });

  it('surfaces native failed responses and retries the requested open operation', async () => {
    const source = port();
    source.setProcessorEditor.mockResolvedValueOnce({
      ...status('failed'),
      failure: 'plugin_rejected',
    });
    const { result } = renderHook(() => useProcessorEditors(source, model()));
    await settle();
    expect(result.current.statuses.get('effect')?.state).toBe('failed');
    expect(result.current.issues.has('effect')).toBe(true);
    act(() => result.current.retry('effect'));
    await settle();
    expect(source.setProcessorEditor.mock.calls).toEqual([
      ['effect', true, 1],
      ['effect', true, 1],
    ]);
    expect(result.current.issues.size).toBe(0);
  });

  it('stops automatic polling on transport failure without pretending the window closed', async () => {
    vi.useFakeTimers();
    const source = port();
    source.getProcessorEditor
      .mockResolvedValueOnce(status('open'))
      .mockRejectedValue(new Error('offline'));
    const { result } = renderHook(() => useProcessorEditors(source, model()));
    await settle();
    await act(async () => vi.advanceTimersByTimeAsync(EDITOR_POLL_MS * 4));
    expect(source.getProcessorEditor).toHaveBeenCalledTimes(2);
    expect(result.current.issues.has('effect')).toBe(true);
    expect(result.current.statuses.get('effect')?.state).toBe('open');
    source.getProcessorEditor.mockResolvedValue(status('closed'));
    act(() => result.current.retry('effect'));
    await settle();
    expect(result.current.issues.size).toBe(0);
    expect(result.current.statuses.get('effect')?.state).toBe('closed');
    expect(source.setProcessorEditor).not.toHaveBeenCalled();
    act(() => result.current.retry('effect'));
    await settle();
    expect(source.setProcessorEditor).toHaveBeenCalledWith('effect', true, 1);
    expect(result.current.statuses.get('effect')?.state).toBe('open');
  });
});

describe('processor editor automatic presentation', () => {
  it('keeps a native-window close closed until an explicit reopen, including graph edits', async () => {
    vi.useFakeTimers();
    const source = port();
    source.getProcessorEditor.mockResolvedValue(status('closed'));
    const { result, rerender } = renderHook(({ graph }) => useProcessorEditors(source, graph), {
      initialProps: { graph: model() },
    });
    await settle();
    expect(source.setProcessorEditor).toHaveBeenCalledTimes(1);
    expect(result.current.statuses.get('effect')?.state).toBe('open');

    await act(async () => vi.advanceTimersByTimeAsync(EDITOR_POLL_MS));
    expect(result.current.statuses.get('effect')?.state).toBe('closed');
    expect(source.setProcessorEditor).toHaveBeenCalledTimes(1);

    const edited = model();
    edited.revision = 2;
    rerender({ graph: edited });
    await settle();
    expect(source.setProcessorEditor).toHaveBeenCalledTimes(1);
    expect(result.current.statuses.get('effect')?.state).toBe('closed');

    act(() => result.current.retry('effect'));
    await settle();
    expect(source.setProcessorEditor).toHaveBeenCalledTimes(2);
    expect(result.current.statuses.get('effect')?.state).toBe('open');
  });

  it('opens another graph with identical local node IDs after a failed editor', async () => {
    const source = port();
    source.getProcessorEditor.mockResolvedValueOnce(status('failed'));
    const { result, rerender } = renderHook(
      ({ graphId }) => useProcessorEditors(source, model(), graphId),
      { initialProps: { graphId: 'first' } },
    );
    await settle();
    expect(result.current.issues.has('effect')).toBe(true);
    rerender({ graphId: 'second' });
    await settle();
    expect(result.current.statuses.get('effect')?.state).toBe('open');
    expect(result.current.issues.size).toBe(0);
    expect(source.setProcessorEditor.mock.calls).toEqual([['effect', true, 1]]);
  });

  it('does not retry a native rejected open automatically', async () => {
    vi.useFakeTimers();
    const source = port();
    source.setProcessorEditor.mockResolvedValue(status('closed'));
    const { result } = renderHook(() => useProcessorEditors(source, model()));
    await settle();
    await act(async () => vi.advanceTimersByTimeAsync(EDITOR_POLL_MS * 4));
    expect(source.setProcessorEditor).toHaveBeenCalledTimes(1);
    expect(result.current.issues.has('effect')).toBe(true);
  });

  it('preserves an already open native editor across graph edits', async () => {
    const source = port();
    source.getProcessorEditor.mockResolvedValue(status('open'));
    const { rerender } = renderHook(({ graph }) => useProcessorEditors(source, graph), {
      initialProps: { graph: model() },
    });
    await settle();
    const edited = model();
    edited.revision = 2;
    edited.nodes = edited.nodes.map((node) => ({ ...node, position: { x: 20, y: 10 } }));
    rerender({ graph: edited });
    await settle();
    expect(source.getProcessorEditor).toHaveBeenCalledTimes(2);
    expect(source.setProcessorEditor).not.toHaveBeenCalled();
  });
});

it('rejects a mismatched native node identity and leaves a retryable issue', async () => {
  const source = port();
  source.getProcessorEditor.mockResolvedValueOnce({ ...status('open'), nodeId: 'unrelated' });
  const { result } = renderHook(() => useProcessorEditors(source, model()));
  await settle();
  expect(result.current.statuses.size).toBe(0);
  expect(result.current.issues.has('effect')).toBe(true);
  act(() => result.current.retry('effect'));
  await settle();
  expect(result.current.statuses.get('effect')?.state).toBe('open');
});
it('ignores a retry after disposal or for an unknown node while preserving unavailable native state', async () => {
  const source = port();
  source.getProcessorEditor.mockResolvedValue(status('unavailable'));
  const graph = model();
  graph.nodes = graph.nodes.map((node) => ({ ...node, runtime: { type: 'provider', config: {} } }));
  const { result, unmount } = renderHook(() => useProcessorEditors(source, graph));
  await settle();
  expect(result.current.statuses.get('effect')?.state).toBe('unavailable');
  act(() => result.current.retry('unknown'));
  const retry = result.current.retry;
  unmount();
  act(() => retry('effect'));
  expect(source.getProcessorEditor).toHaveBeenCalledTimes(1);
  expect(source.setProcessorEditor).not.toHaveBeenCalled();
});

it('keeps an unloaded graph free of editor requests', async () => {
  const source = port();
  const { result } = renderHook(() => useProcessorEditors(source, null));
  await settle();
  expect(result.current.statuses.size).toBe(0);
  expect(source.getProcessorEditor).not.toHaveBeenCalled();
  expect(source.setProcessorEditor).not.toHaveBeenCalled();
});

function withLayout(layoutId: string, revision: number): AudioGraphModel {
  const base = model();
  return {
    ...base,
    revision,
    nodes: base.nodes.map((node) => ({
      ...node,
      runtime: { type: 'provider', config: { typeId: 'vst3:one', ports: { layoutId } } },
    })),
  };
}

/** Re-renders with the next graph and lets every resulting poll and command settle. */
async function step(view: { rerender: (props: never) => void }, props: unknown) {
  view.rerender(props as never);
  await settle();
  await settle();
}

describe('a replaced native instance (new channel layout)', () => {
  it.each([
    ['reopens an editor that was open', 'open', 2],
    ['leaves a closed editor closed', 'closed', 1],
  ] as const)('%s', async (_name, afterOpen, opens) => {
    const source = port();
    source.setProcessorEditor.mockResolvedValue(status(afterOpen));
    const view = renderHook(({ graph }) => useProcessorEditors(source, graph), {
      initialProps: { graph: withLayout('stereo', 1) },
    });
    await settle();
    await settle();
    expect(source.setProcessorEditor).toHaveBeenCalledTimes(1);
    await step(view, { graph: withLayout('mono', 2) });
    expect(source.setProcessorEditor).toHaveBeenCalledTimes(opens);
  });

  it('keeps the reopen intent while the old instance still answers before the apply lands', async () => {
    const source = port();
    source.setProcessorEditor.mockResolvedValue(status('open'));
    const view = renderHook(({ graph }) => useProcessorEditors(source, graph), {
      initialProps: { graph: withLayout('stereo', 1) },
    });
    await settle();
    await settle();
    expect(source.setProcessorEditor).toHaveBeenCalledTimes(1);
    // The new layout reaches the model first; the old native instance is still open.
    source.getProcessorEditor.mockResolvedValue(status('open'));
    await step(view, { graph: withLayout('mono', 1) });
    expect(source.setProcessorEditor).toHaveBeenCalledTimes(1);
    // The applied document arrives; its replacement instance starts closed.
    source.getProcessorEditor.mockResolvedValue(status('closed'));
    await step(view, { graph: withLayout('mono', 2) });
    expect(source.setProcessorEditor).toHaveBeenCalledTimes(2);
  });

  it('forgets the intent for a node that is removed', async () => {
    const source = port();
    source.setProcessorEditor.mockResolvedValue(status('open'));
    const view = renderHook(({ graph }) => useProcessorEditors(source, graph), {
      initialProps: { graph: withLayout('stereo', 1) },
    });
    await settle();
    await settle();
    view.rerender({ graph: withLayout('mono', 1) });
    await settle();
    view.rerender({ graph: { ...withLayout('mono', 2), nodes: [] } });
    await settle();
    await step(view, { graph: withLayout('mono', 3) });
    // Re-added later it is a new node and opens once, not twice.
    expect(source.setProcessorEditor).toHaveBeenCalledTimes(2);
  });

  // Statuses carry no instance identity, so a closed reply before the applied revision may
  // already be the replacement; the editor that was open is reopened rather than lost.
  it('reopens even when the replacement answers closed before the applied revision', async () => {
    const source = port();
    source.setProcessorEditor.mockResolvedValue(status('open'));
    const view = renderHook(({ graph }) => useProcessorEditors(source, graph), {
      initialProps: { graph: withLayout('stereo', 1) },
    });
    await settle();
    await settle();
    source.getProcessorEditor.mockResolvedValue(status('closed'));
    await step(view, { graph: withLayout('mono', 1) });
    await step(view, { graph: withLayout('mono', 2) });
    expect(source.setProcessorEditor).toHaveBeenCalledTimes(2);
  });

  it('does not carry an intent to another graph with the same node ID', async () => {
    const source = port();
    source.setProcessorEditor.mockResolvedValue(status('open'));
    const view = renderHook(({ graph, id }) => useProcessorEditors(source, graph, id), {
      initialProps: { graph: withLayout('stereo', 5), id: 'first' },
    });
    await settle();
    await settle();
    source.getProcessorEditor.mockResolvedValue(status('open'));
    await step(view, { graph: withLayout('mono', 5), id: 'first' });
    // Another graph opens its node once as new; it is not reopened later by the old intent.
    source.getProcessorEditor.mockResolvedValue(status('closed'));
    source.setProcessorEditor.mockResolvedValue(status('closed'));
    await step(view, { graph: withLayout('mono', 1), id: 'second' });
    const opened = source.setProcessorEditor.mock.calls.length;
    await step(view, { graph: withLayout('mono', 6), id: 'second' });
    expect(source.setProcessorEditor).toHaveBeenCalledTimes(opened);
  });
});
