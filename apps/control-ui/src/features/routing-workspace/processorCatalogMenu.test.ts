import { describe, expect, it } from 'vitest';
import type {
  ExternalProcessorDescriptor,
  ProviderPorts,
} from '../../core/domain/control/processors';
import { i18n } from '../../core/i18n';
import {
  createProcessorMenuItems,
  initialProcessorLayout,
  processorCatalogPlaceholder,
  processorLayoutDescription,
  processorLayoutLabel,
  processorSelectionForMenuItem,
} from './processorCatalogMenu';

function stereoLayout(layoutId: string): ProviderPorts {
  const main = (direction: 'Input' | 'Output') => ({
    id: 'audio',
    name: `Main ${direction}`,
    kind: 'main' as const,
    channels: 2,
    channelFormat: 'stereo' as const,
  });
  return { layoutId, inputs: [main('Input')], outputs: [main('Output')] };
}

it('lists every format as sorted direct leaves with distinguishable identities', () => {
  const formats = ['au', 'auv3', 'vst3', 'clap', 'lv2'] as const;
  const processors: ExternalProcessorDescriptor[] = formats.flatMap((format) =>
    ['Zeta', 'Alpha'].map((displayName) => ({
      format,
      displayName,
      typeId: `${format}:${displayName}`,
      vendor: 'Test',
      version: '1',
      layouts: [stereoLayout('layout-v1:stereo')],
      defaultLayoutId: 'layout-v1:stereo',
      hasEditor: true,
      parameters: [],
    })),
  );
  const items = createProcessorMenuItems(processors, i18n.t.bind(i18n), 'Empty');
  expect(items).toHaveLength(10);
  expect(items.map((item) => item.label)).toEqual([
    'Alpha',
    'Alpha',
    'Alpha',
    'Alpha',
    'Alpha',
    'Zeta',
    'Zeta',
    'Zeta',
    'Zeta',
    'Zeta',
  ]);
  expect(new Set(items.map((item) => item.id)).size).toBe(10);
  for (const item of items) {
    expect(item.kind).toBeUndefined();
    expect(item.children).toBeUndefined();
    const selected = processorSelectionForMenuItem(item.id, processors);
    expect(selected).not.toBeNull();
    expect(item.id).toBe(`processor:${selected?.processor.typeId}`);
    expect(item.description).toBe(
      `${i18n.t(`routing.groups.${selected?.processor.format}`)} · Test · Stereo`,
    );
    expect(selected?.ports.layoutId).toBe('layout-v1:stereo');
  }
  expect(processors[0]?.displayName).toBe('Zeta');
  expect(createProcessorMenuItems([], i18n.t.bind(i18n), 'Scanning')).toEqual([
    { disabled: true, id: 'no-processors', label: 'Scanning' },
  ]);
});

it('lists a multi-layout plug-in once and starts it stereo even when another layout is the default', () => {
  const stereo = stereoLayout('layout-v1:sha256:stereo');
  const sidechain: ProviderPorts = {
    layoutId: 'layout-v1:sha256:sidechain',
    inputs: [
      {
        id: 'audio',
        name: 'Main Input',
        kind: 'main',
        channels: 2,
        channelFormat: 'stereo',
      },
      {
        id: 'bus:1',
        name: 'Detector',
        kind: 'sidechain',
        channels: 2,
        channelFormat: 'stereo',
      },
    ],
    outputs: stereo.outputs,
  };
  const processor: ExternalProcessorDescriptor = {
    typeId: 'vst3:vendor:effect',
    displayName: 'Effect',
    vendor: 'Vendor',
    version: '1',
    format: 'vst3',
    hasEditor: true,
    parameters: [],
    layouts: [sidechain, stereo],
    defaultLayoutId: sidechain.layoutId,
  };
  const items = createProcessorMenuItems([processor], i18n.t.bind(i18n), 'Empty');
  expect(items).toEqual([
    { id: 'processor:vst3:vendor:effect', label: 'Effect', description: 'VST3 · Vendor · Stereo' },
  ]);
  expect(processorSelectionForMenuItem(items[0]?.id ?? '', [processor])).toEqual({
    processor,
    ports: stereo,
  });
  expect(processorSelectionForMenuItem('processor:unknown', [processor])).toBeNull();
  // Without a plain stereo layout the plug-in's own default is used, then the first one.
  const noStereo = { ...processor, layouts: [sidechain] };
  expect(initialProcessorLayout(noStereo)).toEqual(sidechain);
  const mono = {
    ...stereo,
    layoutId: 'mono',
    inputs: [{ ...stereo.inputs[0], channels: 1, channelFormat: 'mono' as const }],
  } as ProviderPorts;
  expect(
    initialProcessorLayout({
      ...processor,
      layouts: [mono, sidechain],
      defaultLayoutId: 'missing',
    }),
  ).toEqual(mono);
});

it('localizes layout directions and bus roles without showing machine-kind keys', () => {
  const directional: ProviderPorts = {
    layoutId: 'fingerprint-directional',
    inputs: [
      {
        id: 'input-main',
        name: 'Program',
        kind: 'main',
        channels: 1,
        channelFormat: 'mono',
      },
      {
        id: 'input-sidechain',
        name: 'Detector',
        kind: 'sidechain',
        channels: 2,
        channelFormat: 'stereo',
      },
    ],
    outputs: [
      {
        id: 'output-main',
        name: 'Program',
        kind: 'main',
        channels: 2,
        channelFormat: 'stereo',
      },
    ],
  };
  const processor: ExternalProcessorDescriptor = {
    typeId: 'vst3:directional',
    displayName: 'Effect',
    vendor: 'Test',
    version: '1',
    format: 'vst3',
    hasEditor: false,
    layouts: [directional, stereoLayout('fingerprint-stereo')],
    defaultLayoutId: directional.layoutId,
    parameters: [],
  };
  const ko = i18n.getFixedT('ko-KR');
  expect(processorLayoutLabel(directional, ko)).toBe(
    '모노 입력 → 스테레오 출력 + 사이드체인 2채널',
  );
  expect(processorLayoutDescription(directional, ko)).toContain('입력:');
  const items = createProcessorMenuItems([processor], ko, '없음');
  expect(JSON.stringify(items).toLowerCase()).not.toContain('sidechain');
});

it('rejects duplicate layout identities instead of selecting the default or first duplicate', () => {
  const first = stereoLayout('duplicate:opaque-id');
  const processor: ExternalProcessorDescriptor = {
    typeId: 'vst3:vendor:duplicate',
    displayName: 'Duplicate',
    vendor: 'Vendor',
    version: '1',
    format: 'vst3',
    hasEditor: false,
    layouts: [first, { ...first, inputs: [] }],
    defaultLayoutId: first.layoutId,
    parameters: [],
  };
  expect(createProcessorMenuItems([processor], i18n.t.bind(i18n), 'Empty')).toEqual([]);
  expect(processorSelectionForMenuItem(`processor:${processor.typeId}`, [processor])).toBeNull();
});

describe('empty processor menu evidence', () => {
  it('does not claim there are no installed effects during a scan', () => {
    expect(processorCatalogPlaceholder({ busy: true, failure: null, diagnostics: [] })).toBe(
      'routing.processorControl.scanning',
    );
  });
  it('does not claim there are no installed effects after transport or native failure', () => {
    expect(processorCatalogPlaceholder({ busy: false, failure: 'catalog', diagnostics: [] })).toBe(
      'routing.processorControl.incomplete',
    );
    expect(
      processorCatalogPlaceholder({
        busy: false,
        failure: null,
        diagnostics: [{ format: 'vst3', stage: 'scan', message: 'Timed out' }],
      }),
    ).toBe('routing.processorControl.incomplete');
  });
  it('reports no effects only for a completed clean scan', () => {
    expect(processorCatalogPlaceholder({ busy: false, failure: null, diagnostics: [] })).toBe(
      'routing.noProcessors',
    );
  });
});

it('starts stereo only from one stereo main bus each way, not dual mono or a discrete pair', () => {
  const bus = (
    id: string,
    channels: number,
    channelFormat: ProviderPorts['inputs'][number]['channelFormat'],
  ) => ({
    id,
    name: id,
    kind: 'main' as const,
    channels,
    channelFormat,
  });
  const dualMono: ProviderPorts = {
    layoutId: 'dual-mono',
    inputs: [bus('l', 1, 'mono'), bus('r', 1, 'mono')],
    outputs: [bus('l', 1, 'mono'), bus('r', 1, 'mono')],
  };
  const discrete: ProviderPorts = {
    layoutId: 'discrete-2',
    inputs: [bus('audio', 2, 'discrete')],
    outputs: [bus('audio', 2, 'discrete')],
  };
  const stereo = stereoLayout('stereo');
  const processor: ExternalProcessorDescriptor = {
    typeId: 'au:pair',
    displayName: 'Pair',
    vendor: 'Vendor',
    version: '1',
    format: 'au',
    hasEditor: false,
    parameters: [],
    layouts: [dualMono, discrete, stereo],
    defaultLayoutId: 'dual-mono',
  };
  expect(initialProcessorLayout(processor)).toEqual(stereo);
  expect(initialProcessorLayout({ ...processor, layouts: [dualMono, discrete] })).toEqual(dualMono);
});
