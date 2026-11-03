import { describe, expect, it } from 'vitest';
import type {
  ExternalProcessorDescriptor,
  ProviderPorts,
} from '../../core/domain/control/processors';
import { i18n } from '../../core/i18n';
import {
  createProcessorMenuItems,
  processorLayoutLabel,
  processorSelectionForMenuItem,
} from './processorCatalogMenu';

const t = i18n.getFixedT('en');
const main = (
  channels: number,
  channelFormat: ProviderPorts['inputs'][number]['channelFormat'],
  id = 'audio',
) => ({ id, name: id, kind: 'main' as const, channels, channelFormat });
const stereo: ProviderPorts = {
  layoutId: 'stereo',
  inputs: [main(2, 'stereo')],
  outputs: [main(2, 'stereo')],
};
function processor(change: Partial<ExternalProcessorDescriptor> = {}): ExternalProcessorDescriptor {
  return {
    typeId: 'fx:one',
    displayName: 'Effect',
    vendor: 'Vendor',
    version: '1',
    format: 'vst3',
    layouts: [stereo],
    defaultLayoutId: 'stereo',
    hasEditor: true,
    parameters: [],
    ...change,
  };
}

it.each([
  [main(1, 'mono'), 'Mono'],
  [main(2, 'stereo'), 'Stereo'],
  [main(6, 'surround5_1'), '5.1'],
  [main(8, 'surround7_1'), '7.1'],
  [main(12, 'discrete'), '12 ch'],
])('labels an authored channel format %j', (bus, expected) => {
  expect(processorLayoutLabel({ layoutId: 'channels', inputs: [bus], outputs: [bus] }, t)).toBe(
    expected,
  );
});

it('distinguishes input-only, output-only and portless topologies without an invented direction', () => {
  expect(processorLayoutLabel({ ...stereo, inputs: [] }, t)).toBe('Stereo');
  expect(processorLayoutLabel({ ...stereo, outputs: [] }, t)).toBe('Stereo');
  expect(processorLayoutLabel({ layoutId: 'none', inputs: [], outputs: [] }, t)).toBe(
    'No audio ports',
  );
});

it('sums multiple main buses and explicitly counts multiple sidechain and auxiliary buses', () => {
  const ports: ProviderPorts = {
    layoutId: 'multi-bus',
    inputs: [
      main(2, 'stereo', 'main-a'),
      main(2, 'stereo', 'main-b'),
      { ...main(1, 'mono', 'side-a'), kind: 'sidechain' },
      { ...main(1, 'mono', 'side-b'), kind: 'sidechain' },
    ],
    outputs: [
      main(4, 'discrete'),
      { ...main(2, 'stereo', 'aux-a'), kind: 'auxiliary' },
      { ...main(2, 'stereo', 'aux-b'), kind: 'auxiliary' },
    ],
  };
  expect(processorLayoutLabel(ports, t)).toBe(
    '4 ch + 2 Sidechain buses · 2 ch + 2 Auxiliary buses · 4 ch',
  );
});

it('excludes instrument and analyzer layouts missing an authored audio direction', () => {
  const inputOnly = { ...stereo, layoutId: 'input', outputs: [] };
  const outputOnly = { ...stereo, layoutId: 'output', inputs: [] };
  const fx = processor({ layouts: [inputOnly, outputOnly] });
  expect(createProcessorMenuItems([fx], t, 'Empty')).toEqual([]);
  expect(processorSelectionForMenuItem('processor:fx:one', [fx])).toBeNull();
});

describe('deterministic plugin identity sorting and selection', () => {
  it('orders equal display names and formats by vendor and opaque type identity', () => {
    const processors = [
      processor({ typeId: 'fx:z', vendor: 'Zulu' }),
      processor({ typeId: 'fx:b' }),
      processor({ typeId: 'fx:a' }),
    ];
    expect(createProcessorMenuItems(processors, t, 'Empty').map((item) => item.id)).toEqual([
      'processor:fx:a',
      'processor:fx:b',
      'processor:fx:z',
    ]);
    expect(processors[0]?.typeId).toBe('fx:z');
  });
  it('selects a single-layout descriptor by its plug-in identity', () => {
    const fx = processor();
    expect(processorSelectionForMenuItem('processor:fx:one', [fx])).toEqual({
      ports: stereo,
      processor: fx,
    });
    expect(processorSelectionForMenuItem('processor:missing', [fx])).toBeNull();
  });
  it('drops malformed layouts and keeps remaining valid selections exact', () => {
    const invalid = { ...stereo, layoutId: 'invalid', inputs: [main(0, 'discrete')] };
    const fx = processor({ layouts: [invalid, stereo] });
    const items = createProcessorMenuItems([fx], t, 'Empty');
    expect(items).toHaveLength(1);
    expect(processorSelectionForMenuItem('processor:fx:one', [fx])).toEqual({
      ports: stereo,
      processor: fx,
    });
    expect(createProcessorMenuItems([processor({ layouts: [invalid] })], t, 'Empty')).toEqual([]);
  });
});
