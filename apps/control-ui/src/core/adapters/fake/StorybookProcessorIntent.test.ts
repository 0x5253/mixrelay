import { describe, expect, it } from 'vitest';
import { ControlPortError } from '../../domain/control/errors';
import type { RuntimeGraphDocument } from '../../domain/control/graph';
import { StorybookMockControlPort } from './StorybookMockControlPort';

function graph(): RuntimeGraphDocument {
  return {
    schemaVersion: 2,
    revision: 1,
    edges: [],
    nodes: [
      {
        id: 'fx',
        type: 'provider',
        label: null,
        config: { typeId: 'vst3:fx', parameters: { parameterValues: { '0': 0.8 } } },
      },
      { id: 'source', type: 'application_capture', label: null, config: null },
    ],
  };
}

function fixture() {
  return new StorybookMockControlPort({
    graph: graph(),
    graphStatus: { protocolVersion: 1, activeRevision: 1, submittedRevision: 1 },
    status: {
      protocolVersion: 1,
      driverState: 'ready',
      sampleRate: 48000,
      bufferFrames: 128,
      underruns: 0,
      overruns: 0,
    },
  });
}

const change = { nodeId: 'fx', typeId: 'vst3:fx', parameterId: 0, value: 0.5 };

describe('processor intent validation in the interactive preview', () => {
  it.each([
    { nodeId: 'missing' },
    { nodeId: 'source' },
    { typeId: 'vst3:other' },
    { parameterId: -1 },
    { parameterId: 0.5 },
    { value: Number.NaN },
    { value: -0.1 },
    { value: 1.1 },
  ])('rejects invalid intent %j without committing other graph changes', async (override) => {
    const port = fixture();
    await expect(
      port.applyGraph({
        ...graph(),
        revision: 2,
        processorParameterChanges: [{ ...change, ...override }],
      }),
    ).rejects.toThrow('graph_invalid');
    expect(await port.getGraph()).toEqual(graph());
  });

  it.each(['removed', 'non-provider', 'replaced-instance'] as const)(
    'rejects an intent whose destination was %s',
    async (destination) => {
      const port = fixture();
      const request = graph();
      if (destination === 'removed')
        request.nodes = request.nodes.filter((node) => node.id !== 'fx');
      else {
        const target = request.nodes[0];
        if (!target) throw new Error('Missing provider fixture');
        if (destination === 'non-provider') target.type = 'gain';
        else target.config = { typeId: 'vst3:new' };
      }
      await expect(
        port.applyGraph({ ...request, revision: 2, processorParameterChanges: [change] }),
      ).rejects.toThrow('graph_invalid');
      expect((await port.getGraph()).revision).toBe(1);
    },
  );

  it('enforces the intent batch budget before replacing the graph', async () => {
    const port = fixture();
    const changes = Array.from({ length: 4097 }, (_, parameterId) => ({ ...change, parameterId }));
    await expect(
      port.applyGraph({ ...graph(), revision: 2, processorParameterChanges: changes }),
    ).rejects.toThrow('graph_invalid');
    expect((await port.getGraph()).revision).toBe(1);
  });

  it('preserves unchanged provider values while allowing new and replaced providers to initialize their own state', async () => {
    const port = fixture();
    const request = graph();
    request.nodes.push({
      id: 'new',
      type: 'provider',
      label: null,
      config: { typeId: 'vst3:new' },
    });
    await port.applyGraph({ ...request, revision: 2, processorParameterChanges: [change] });
    expect((await port.getGraph()).nodes[0]?.config).toMatchObject({
      parameters: { parameterValues: { '0': 0.5 } },
    });
    const replaced = graph();
    const fx = replaced.nodes[0];
    if (!fx) throw new Error('Missing provider fixture');
    fx.config = { typeId: 'vst3:replacement', parameters: { parameterValues: { '0': 0.2 } } };
    await port.applyGraph({ ...replaced, revision: 3 });
    expect((await port.getGraph()).nodes[0]?.config).toEqual(fx.config);
  });

  it.each([null, [], { typeId: 'vst3:fx', parameters: [] }])(
    'handles provider state that has no persisted parameter record %j',
    async (config) => {
      const port = fixture();
      const request = graph();
      const fx = request.nodes[0];
      if (!fx) throw new Error('Missing provider fixture');
      fx.config = config;
      await port.applyGraph({ ...request, revision: 2 });
      await expect(port.setProcessorParameter('fx', 0, 0.5)).resolves.toMatchObject({ value: 0.5 });
      expect((await port.getGraph()).nodes[0]?.config).toMatchObject({
        parameters: { parameterValues: { '0': 0.5 } },
      });
    },
  );
});

it('keeps independent providers unchanged when one provider receives explicit parameter intent', async () => {
  const document = graph();
  document.nodes.push({
    id: 'other',
    type: 'provider',
    label: null,
    config: { typeId: 'vst3:other', parameters: { parameterValues: { '0': 0.9 } } },
  });
  const port = new StorybookMockControlPort({
    graph: document,
    graphStatus: { protocolVersion: 1, activeRevision: 1, submittedRevision: 1 },
    status: {
      protocolVersion: 1,
      driverState: 'ready',
      sampleRate: 48000,
      bufferFrames: 128,
      underruns: 0,
      overruns: 0,
    },
  });
  await port.applyGraph({ ...document, revision: 2, processorParameterChanges: [change] });
  expect((await port.getGraph()).nodes[0]?.config).toMatchObject({
    parameters: { parameterValues: { '0': 0.5 } },
  });
  expect((await port.getGraph()).nodes[2]?.config).toMatchObject({
    parameters: { parameterValues: { '0': 0.9 } },
  });
});

describe('graph targeting in the interactive preview', () => {
  it('rejects a document named for another graph and applies one for the selected graph', async () => {
    const port = fixture();
    const { activeId } = await port.getGraphCatalog();
    const request = { ...graph(), revision: 2, nodes: graph().nodes.slice(0, 1) };
    const rejection = port.applyGraph({ ...request, expectedGraphId: `${activeId}-other` });
    await expect(rejection).rejects.toBeInstanceOf(ControlPortError);
    await expect(rejection).rejects.toMatchObject({
      status: 409,
      body: { code: 'graph_revision_stale' },
    });
    expect(await port.getGraph()).toEqual(graph());
    await port.applyGraph({ ...request, expectedGraphId: activeId });
    expect(await port.getGraph()).toEqual(request);
  });

  it.each([3, 2])(
    'rejects revision %i once revision 3 was submitted, like the engine',
    async (revision) => {
      const port = fixture();
      const accepted = { ...graph(), revision: 3, nodes: graph().nodes.slice(0, 1) };
      await port.applyGraph(accepted);
      const rejection = port.applyGraph({ ...graph(), revision });
      await expect(rejection).rejects.toBeInstanceOf(ControlPortError);
      await expect(rejection).rejects.toMatchObject({
        body: {
          code: 'graph_revision_stale',
          parameters: { activeRevision: 3, submittedRevision: 3, proposedRevision: revision },
        },
      });
      expect(await port.getGraph()).toEqual(accepted);
      expect(await port.getGraphStatus()).toMatchObject({ submittedRevision: 3 });
    },
  );
});
