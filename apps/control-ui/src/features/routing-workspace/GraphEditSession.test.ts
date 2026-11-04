import { expect, it, vi } from 'vitest';
import type { AudioGraphModel } from '../../core/audio-graph/model';
import { GraphEditSession, type GraphEditTargets, type GraphHistoryStep } from './GraphEditSession';
import { updateRoutingNodeParameter } from './model';

const processor: AudioGraphModel = {
  revision: 4,
  edges: [],
  nodes: [
    {
      id: 'p',
      position: { x: 0, y: 0 },
      inputPorts: [],
      outputPorts: [],
      runtime: { type: 'provider', config: { typeId: 'vst3:test' } },
      appearance: {
        icon: 'VST3',
        title: 'Effect',
        category: 'Processor',
        parameterControls: [1, 2].map((id) => ({
          id: `external:${id}`,
          label: String(id),
          value: id / 10,
          minimum: 0,
          maximum: 1,
          step: 0.01,
          suffix: '',
        })),
      },
    },
  ],
};
const moved = (model: AudioGraphModel, x: number): AudioGraphModel => ({
  ...model,
  nodes: model.nodes.map((node) => ({ ...node, position: { x, y: 0 } })),
});

function fixture(canPublishParameters = true) {
  const model = { current: processor as AudioGraphModel | null };
  const targets = {
    history: {
      record: vi.fn(),
      recordDebounced: vi.fn(),
      flush: vi.fn(),
      undo: vi.fn((): GraphHistoryStep | null => null),
      redo: vi.fn((): GraphHistoryStep | null => null),
    },
    queue: { canPublishParameters, submit: vi.fn() },
    publishParameter: vi.fn(),
    graphRevision: () => 9,
    show: vi.fn(),
    savePositions: vi.fn(),
  } satisfies GraphEditTargets;
  const session = new GraphEditSession(model);
  const owner = {};
  session.render(owner, targets);
  session.open(owner);
  return { model, owner, session, targets };
}

it('shows, saves and submits a structural edit as its own Undo step', () => {
  const { model, owner, session, targets } = fixture();
  const next = { ...processor, revision: 5, nodes: [] };
  session.edit(owner, next, 'runtime');
  expect(model.current).toBe(next);
  expect(targets.history.flush).toHaveBeenCalledBefore(targets.history.record);
  expect(targets.history.record).toHaveBeenCalledWith(processor, next, 'runtime', undefined);
  expect(targets.show).toHaveBeenCalledWith(next);
  expect(targets.savePositions).toHaveBeenCalledWith(next);
  expect(targets.queue.submit).toHaveBeenCalledWith(next, []);
});

it('keeps a layout edit local and an unchanged model out of history', () => {
  const { model, owner, session, targets } = fixture();
  session.edit(owner, moved(processor, 40), 'layout');
  expect(targets.history.record).toHaveBeenCalledOnce();
  expect(targets.savePositions).toHaveBeenCalledOnce();
  session.edit(owner, model.current as AudioGraphModel, 'layout');
  expect(targets.history.record).toHaveBeenCalledOnce();
  expect(targets.savePositions).toHaveBeenCalledTimes(2);
  expect(targets.queue.submit).not.toHaveBeenCalled();
  expect(targets.publishParameter).not.toHaveBeenCalled();
});

it('coalesces a continuing control gesture into one pending Undo step', () => {
  const { owner, session, targets } = fixture();
  const next = updateRoutingNodeParameter(processor, 'p', 'external:1', 0.5);
  session.edit(owner, next, 'processor_parameter');
  expect(targets.history.recordDebounced).toHaveBeenCalledWith(
    processor,
    next,
    'processor_parameter',
    expect.any(String),
  );
  expect(targets.history.record).not.toHaveBeenCalled();
});

it('publishes changed processor parameters directly when the queue is idle', () => {
  const { owner, session, targets } = fixture();
  session.edit(
    owner,
    updateRoutingNodeParameter(processor, 'p', 'external:2', 0.7),
    'processor_parameter',
  );
  expect(targets.publishParameter).toHaveBeenCalledExactlyOnceWith({
    nodeId: 'p',
    typeId: 'vst3:test',
    parameterId: 2,
    value: 0.7,
    expectedRevision: 9,
  });
  expect(targets.queue.submit).not.toHaveBeenCalled();
});

it('carries parameter intent with the graph while a graph application is in flight', () => {
  const { owner, session, targets } = fixture(false);
  const next = updateRoutingNodeParameter(processor, 'p', 'external:1', 0.5);
  session.edit(owner, next, 'processor_parameter');
  expect(targets.publishParameter).not.toHaveBeenCalled();
  expect(targets.queue.submit).toHaveBeenCalledWith(next, [
    { nodeId: 'p', typeId: 'vst3:test', parameterId: 1, value: 0.5 },
  ]);
});

it('discards a callback whose owner was replaced, even before the new owner opens', () => {
  const { model, owner, session, targets } = fixture();
  const replacement = {};
  session.render(replacement, targets);
  session.close(owner);
  session.edit(owner, moved(processor, 1), 'runtime');
  session.edit(replacement, moved(processor, 2), 'runtime');
  expect(targets.show).not.toHaveBeenCalled();
  session.open(owner);
  session.edit(owner, moved(processor, 3), 'runtime');
  expect(model.current).toBe(processor);
  session.open(replacement);
  session.edit(owner, moved(processor, 4), 'runtime');
  expect(targets.show).not.toHaveBeenCalled();
  session.edit(replacement, moved(processor, 5), 'runtime');
  expect(targets.queue.submit).toHaveBeenCalledOnce();
});

it('discards edits after its owner closes and accepts them again when it reopens', () => {
  const { owner, session, targets } = fixture();
  session.close(owner);
  session.edit(owner, moved(processor, 1), 'runtime');
  expect(targets.show).not.toHaveBeenCalled();
  session.open(owner);
  session.edit(owner, moved(processor, 2), 'runtime');
  expect(targets.show).toHaveBeenCalledOnce();
});

it('restores an Undo or Redo step as a newer revision, except layout', () => {
  const { model, session, targets } = fixture();
  expect(session.undo()).toBe(false);
  expect(session.redo()).toBe(false);
  targets.history.undo.mockReturnValueOnce({
    value: { ...processor, revision: 2 },
    kind: 'runtime',
  });
  expect(session.undo()).toBe(true);
  expect(model.current?.revision).toBe(5);
  expect(targets.queue.submit).toHaveBeenLastCalledWith(model.current, []);
  targets.history.redo.mockReturnValueOnce({ value: moved(processor, 8), kind: 'layout' });
  expect(session.redo()).toBe(true);
  expect(model.current).toMatchObject({ revision: 5, nodes: [{ position: { x: 8, y: 0 } }] });
  expect(targets.queue.submit).toHaveBeenCalledOnce();
  expect(targets.history.record).not.toHaveBeenCalled();
});

it('restores a processor parameter step through direct publication', () => {
  const { session, targets } = fixture();
  targets.history.undo.mockReturnValueOnce({
    value: updateRoutingNodeParameter(processor, 'p', 'external:1', 0.3),
    kind: 'processor_parameter',
  });
  session.undo();
  expect(targets.publishParameter).toHaveBeenCalledWith(
    expect.objectContaining({ parameterId: 1, value: 0.3, expectedRevision: 9 }),
  );
});

it('a discarded render of another owner leaves the mounted owner editing', () => {
  const { owner, session, targets } = fixture();
  session.render({}, targets);
  session.render(owner, targets);
  session.render(owner, targets);
  session.edit(owner, moved(processor, 1), 'runtime');
  expect(targets.show).toHaveBeenCalledOnce();
});

it('restores no history step before the new owner mounts', () => {
  const { owner, session, targets } = fixture();
  const replacement = {};
  session.render(replacement, targets);
  session.close(owner);
  targets.history.undo.mockReturnValue({ value: processor, kind: 'runtime' });
  expect(session.undo()).toBe(false);
  expect(session.redo()).toBe(false);
  expect(targets.history.undo).not.toHaveBeenCalled();
  session.open(replacement);
  expect(session.undo()).toBe(true);
});
