import { describe, expect, it } from 'vitest';
import {
  connectionPreviewPairs,
  dragConnectionRequest,
  selectedConnectionBatch,
  selectedOutputPorts,
} from './connectionPlanning';
import type { AudioGraphModel, AudioGraphNode } from './model';

function node(id: string, inputs: number, outputs: number): AudioGraphNode {
  const ports = (prefix: string, count: number) =>
    Array.from({ length: count }, (_, index) => ({
      id: `${prefix}${index + 1}`,
      label: `${prefix}${index + 1}`,
    }));
  return {
    id,
    position: { x: 0, y: 0 },
    inputPorts: ports('in', inputs),
    outputPorts: ports('out', outputs),
    appearance: { category: 'Test', icon: 'T', title: id },
  };
}

const model: AudioGraphModel = {
  revision: 1,
  nodes: [node('app', 0, 4), node('out', 3, 0)],
  edges: [],
};
const output = (portId: string, nodeId = 'app') => ({ nodeId, portId, side: 'output' as const });
const grab = (sourcePortId: string, targetPortId = 'in1') => ({
  sourceNodeId: 'app',
  sourcePortId,
  targetNodeId: 'out',
  targetPortId,
});

describe('connection planning', () => {
  it('reads the selected outputs of one node in port order', () => {
    const selected = [
      output('out3'),
      output('out1'),
      output('out2', 'out'),
      { ...output('out4'), side: 'input' as const },
    ];
    expect(selectedOutputPorts(model, selected, 'app').map((port) => port.id)).toEqual([
      'out1',
      'out3',
    ]);
    expect(selectedOutputPorts(model, selected, 'missing')).toEqual([]);
  });

  it('carries the selection when the grabbed port is part of it, else only the grabbed port', () => {
    const selected = [output('out1'), output('out2')];
    expect(dragConnectionRequest(model, grab('out2', 'in2'), selected)).toEqual({
      sourceNodeId: 'app',
      sourcePortIds: ['out1', 'out2'],
      sourceAnchorPortId: 'out2',
      targetNodeId: 'out',
      targetAnchorPortId: 'in2',
    });
    expect(dragConnectionRequest(model, grab('out3'), selected).sourcePortIds).toEqual(['out3']);
    expect(dragConnectionRequest(model, grab('out1'), [output('out1')]).sourcePortIds).toEqual([
      'out1',
    ]);
  });

  it('keeps channel offsets from the anchor and falls back to the grabbed cable', () => {
    const selected = [output('out1'), output('out2')];
    expect(
      selectedConnectionBatch(model, grab('out2', 'in3'), selected).map((pair) => [
        pair.sourcePortId,
        pair.targetPortId,
      ]),
    ).toEqual([
      ['out1', 'in2'],
      ['out2', 'in3'],
    ]);
    const missingTarget = grab('out1', 'gone');
    expect(selectedConnectionBatch(model, missingTarget, selected)).toEqual([missingTarget]);
  });

  it('previews every carried cable, paired with targets once over a target', () => {
    const selected = [output('out1'), output('out2')];
    expect(connectionPreviewPairs(model, selected, 'app', 'out1')).toMatchObject([
      { sourcePort: { id: 'out1' } },
      { sourcePort: { id: 'out2' } },
    ]);
    expect(connectionPreviewPairs(model, selected, 'app', 'out4')).toEqual([
      { sourcePort: { id: 'out4' } },
    ]);
    expect(connectionPreviewPairs(model, selected, 'app', 'out1', 'out', 'in1')).toEqual([
      { sourcePort: { id: 'out1' }, targetPort: { id: 'in1' } },
      { sourcePort: { id: 'out2' }, targetPort: { id: 'in2' } },
    ]);
  });
});
