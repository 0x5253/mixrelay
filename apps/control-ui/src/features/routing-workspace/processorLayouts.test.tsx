import { renderHook } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import type { AudioGraphModel } from '../../core/audio-graph/model';
import type {
  ExternalProcessorDescriptor,
  ProviderPorts,
} from '../../core/domain/control/processors';
import { connectGraphBatch } from '../../core/domain/graph/topology';
import { i18n } from '../../core/i18n';
import { addRoutingNode, changeProcessorLayout, defaultPresentation } from './model';
import { presentProcessorLayouts, useProcessorLayoutChange } from './processorLayouts';

function layout(layoutId: string, channels: number): ProviderPorts {
  const bus = {
    id: 'audio',
    name: 'Main',
    kind: 'main' as const,
    channels,
    channelFormat: channels === 1 ? ('mono' as const) : ('stereo' as const),
  };
  return { layoutId, inputs: [bus], outputs: [bus] };
}

const mono = layout('mono', 1);
const stereo = layout('stereo', 2);
const processor: ExternalProcessorDescriptor = {
  typeId: 'au:apple:bandpass',
  displayName: 'AUBandpass',
  vendor: 'Apple',
  version: '1',
  format: 'au',
  hasEditor: true,
  parameters: [],
  layouts: [mono, stereo],
  defaultLayoutId: 'mono',
};
const t = i18n.getFixedT('en');

/** A stereo plug-in node feeding a recorder on both channels. */
function graph(): AudioGraphModel {
  let model: AudioGraphModel = { revision: 1, nodes: [], edges: [] };
  model = addRoutingNode(model, { kind: 'provider', processor, ports: stereo }, { x: 0, y: 0 });
  model = addRoutingNode(model, { kind: 'recorder' }, { x: 400, y: 0 });
  const [fx, recorder] = model.nodes;
  if (!fx || !recorder) throw new Error('fixture nodes missing');
  return connectGraphBatch(
    model,
    fx.outputPorts.map((port, index) => ({
      sourceNodeId: fx.id,
      sourcePortId: port.id,
      targetNodeId: recorder.id,
      targetPortId: recorder.inputPorts[index]?.id ?? '',
    })),
  );
}

it('offers the catalog layouts on the node, labelled and described', () => {
  const model = graph();
  const presented = presentProcessorLayouts(model, [processor], t);
  expect(presented.nodes[0]?.appearance.channelLayoutControl).toEqual({
    label: 'Channel layout',
    value: 'stereo',
    options: [
      expect.objectContaining({ value: 'mono', label: 'Mono' }),
      expect.objectContaining({ value: 'stereo', label: 'Stereo' }),
    ],
  });
  expect(presented.nodes[1]?.appearance.channelLayoutControl).toBeUndefined();
  // One layout, an unknown plug-in, or a layout the catalog dropped: nothing to choose.
  expect(presentProcessorLayouts(model, [{ ...processor, layouts: [stereo] }], t)).toBe(model);
  expect(presentProcessorLayouts(model, [], t)).toBe(model);
  expect(
    presentProcessorLayouts(model, [{ ...processor, layouts: [mono, layout('5.1', 6)] }], t),
  ).toBe(model);
});

it('disambiguates layouts that share a label', () => {
  const twin = { ...stereo, layoutId: 'stereo-b' };
  const options = presentProcessorLayouts(graph(), [{ ...processor, layouts: [stereo, twin] }], t)
    .nodes[0]?.appearance.channelLayoutControl?.options;
  expect(options?.map((option) => option.label)).toEqual(['Stereo · 1', 'Stereo · 2']);
});

it('switches layout in one runtime edit, keeping cables on channels that remain', () => {
  const model = graph();
  const fx = model.nodes[0];
  if (!fx) throw new Error('fixture node missing');
  expect(model.edges).toHaveLength(2);
  const updateModel = vi.fn();
  const change = renderHook(() =>
    useProcessorLayoutChange(model, [processor], defaultPresentation, updateModel),
  ).result.current;
  change(fx.id, 'mono');
  expect(updateModel).toHaveBeenCalledOnce();
  const [next, kind] = updateModel.mock.calls[0] as [AudioGraphModel, string];
  expect(kind).toBe('runtime');
  const switched = next.nodes[0];
  expect(switched?.outputPorts).toHaveLength(1);
  expect(switched?.inputPorts).toHaveLength(1);
  expect(switched?.appearance.channelCount).toBe(1);
  expect(switched?.runtime?.config).toMatchObject({ ports: mono, typeId: processor.typeId });
  // CH 1 keeps its cable and port ID; CH 2 is gone with its cable.
  expect(next.edges).toEqual([model.edges[0]]);
  // Unknown layouts, other node types and a missing model change nothing.
  change(fx.id, 'missing');
  change(model.nodes[1]?.id ?? '', 'mono');
  renderHook(() =>
    useProcessorLayoutChange(null, [processor], defaultPresentation, updateModel),
  ).result.current(fx.id, 'mono');
  expect(updateModel).toHaveBeenCalledOnce();
});

it('leaves the model unchanged for the current layout or another plug-in', () => {
  const model = graph();
  const id = model.nodes[0]?.id ?? '';
  expect(changeProcessorLayout(model, id, processor, stereo, defaultPresentation)).toBe(model);
  const other = { ...processor, typeId: 'au:other' };
  expect(changeProcessorLayout(model, id, other, mono, defaultPresentation)).toBe(model);
  expect(changeProcessorLayout(model, 'missing', processor, mono, defaultPresentation)).toBe(model);
});

it('does not reapply the graph when the current layout is chosen again', () => {
  const model = graph();
  const updateModel = vi.fn();
  renderHook(() =>
    useProcessorLayoutChange(model, [processor], defaultPresentation, updateModel),
  ).result.current(model.nodes[0]?.id ?? '', 'stereo');
  expect(updateModel).not.toHaveBeenCalled();
});
