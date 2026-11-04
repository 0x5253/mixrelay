import { describe, expect, it } from 'vitest';
import { spacing } from '../theme/tokens';
import type { AudioNodeParameterControl } from './AudioNodeParameterGrid';
import {
  audioNodeCardStyle,
  bypassControl,
  bypassLabel,
  cardStateAttributes,
  compactFeedbackMessage,
  ioSummaryText,
  meterBanksForCompactFace,
  nodeOptionsAvailability,
  offersProcessorEditorAction,
  partitionParameterControls,
  prefersProcessorEditor,
} from './audioNodeCardModel';
import { audioNodeFaceGeometry, audioNodeFrame } from './audioNodeLayout';

function control(
  id: string,
  overrides: Partial<AudioNodeParameterControl> = {},
): AudioNodeParameterControl {
  return {
    id,
    label: id,
    minimum: 0,
    maximum: 1,
    step: 1,
    suffix: '',
    value: 0,
    ...overrides,
  };
}

const change = () => {};
const layout = {
  label: 'Layout',
  value: 'stereo',
  options: [
    { value: 'mono', label: 'Mono' },
    { value: 'stereo', label: 'Stereo' },
  ],
};

describe('node options availability', () => {
  it('offers nothing without a setting that also has an owner', () => {
    expect(nodeOptionsAvailability({ gainControl: { label: 'Gain', valueDb: 0 } })).toEqual({
      hasOptions: false,
      channelLayout: undefined,
      hasEndpointControl: false,
      hasParameterControls: false,
    });
  });

  it('offers gain, endpoint and parameters once each has its owner', () => {
    expect(
      nodeOptionsAvailability({ gainControl: { label: 'Gain', valueDb: 0 }, onGainChange: change })
        .hasOptions,
    ).toBe(true);
    const endpoint = nodeOptionsAvailability({
      endpointControl: {} as never,
      onEndpointChange: change,
    });
    expect([endpoint.hasOptions, endpoint.hasEndpointControl]).toEqual([true, true]);
    const parameters = nodeOptionsAvailability({
      parameterControls: [control('mix')],
      onParameterChange: change,
    });
    expect([parameters.hasOptions, parameters.hasParameterControls]).toEqual([true, true]);
  });

  it('offers a channel layout only when there is more than one choice', () => {
    const offered = nodeOptionsAvailability({
      channelLayoutControl: layout,
      onChannelLayoutChange: change,
    });
    expect(offered.hasOptions).toBe(true);
    expect(offered.channelLayout).toEqual({ control: layout, onChange: change });
    const single = { ...layout, options: layout.options.slice(0, 1) };
    expect(
      nodeOptionsAvailability({ channelLayoutControl: single, onChannelLayoutChange: change })
        .channelLayout,
    ).toBeUndefined();
  });

  it('lets a working native editor replace the generic parameter list', () => {
    const parameters = { parameterControls: [control('mix')], onParameterChange: change };
    for (const state of ['closed', 'opening', 'open'] as const)
      expect(
        nodeOptionsAvailability({ ...parameters, processorEditor: { actionLabel: '', state } })
          .hasParameterControls,
      ).toBe(false);
    for (const state of ['failed', 'unavailable'] as const)
      expect(
        nodeOptionsAvailability({ ...parameters, processorEditor: { actionLabel: '', state } })
          .hasParameterControls,
      ).toBe(true);
  });
});

it('prefers and offers the native editor by its state', () => {
  expect(prefersProcessorEditor(undefined)).toBe(false);
  expect(prefersProcessorEditor({ actionLabel: '', state: 'open' })).toBe(true);
  expect(prefersProcessorEditor({ actionLabel: '', state: 'failed' })).toBe(false);
  const offered = (['closed', 'failed', 'opening', 'open', 'unavailable'] as const).filter(
    (state) => offersProcessorEditorAction({ actionLabel: '', state }),
  );
  expect(offered).toEqual(['closed', 'failed']);
});

it('keeps bypass in the header and option parameters only for an editable node', () => {
  const controls = [
    control('bypass'),
    control('mix'),
    control('drive', { section: 'main' }),
    control('oversampling', { section: 'options' }),
  ];
  const editable = partitionParameterControls(controls, true);
  expect(editable.main.map((item) => item.id)).toEqual(['mix', 'drive']);
  expect(editable.options.map((item) => item.id)).toEqual(['oversampling']);
  expect(partitionParameterControls(controls, false).options).toEqual([]);
  expect(bypassControl(controls)?.id).toBe('bypass');
  expect(bypassControl([control('mix')])).toBeUndefined();
});

it('names bypass by its current choice, or its raw value without choices', () => {
  const choices = [
    { value: 0, label: 'Active' },
    { value: 1, label: 'Bypassed' },
  ];
  expect(bypassLabel('EQ', control('bypass', { label: 'Bypass', value: 1, choices }))).toBe(
    'EQ: Bypass: Bypassed',
  );
  expect(bypassLabel('EQ', control('bypass', { label: 'Bypass', value: 1 }))).toBe('EQ: Bypass: 1');
});

it('summarizes channels as IN/OUT only when both directions differ', () => {
  expect(ioSummaryText(2, 6, 0)).toBe('IN 2 / OUT 6');
  expect(ioSummaryText(2, 2, 0)).toBe('2 CH');
  expect(ioSummaryText(0, 6, 8)).toBe('8 CH');
});

it('shows compact detail, or a critical status in its place', () => {
  expect(compactFeedbackMessage('standard', 'Detail', 'Status', 'critical')).toBeUndefined();
  expect(compactFeedbackMessage('compact', 'Detail', 'Status', 'neutral')).toBe('Detail');
  expect(compactFeedbackMessage('compact', 'Detail', 'Status', 'critical')).toBe('Status');
  expect(compactFeedbackMessage('compact', 'Detail', '', 'critical')).toBe('Detail');
});

it('moves meters into the controls only for a compact face', () => {
  const banks = [{ channels: 2, label: 'In', levels: [] }] as never[];
  expect(meterBanksForCompactFace('compact', banks)).toBe(banks);
  expect(meterBanksForCompactFace('standard', banks)).toEqual([]);
});

it('marks only the active card states', () => {
  expect(cardStateAttributes({ poweredOff: false })).toEqual({
    'data-bypassed': undefined,
    'data-offline': undefined,
    'data-powered-off': false,
    'data-selected': undefined,
    'data-dynamics': undefined,
  });
  expect(
    cardStateAttributes({
      bypassed: true,
      offline: true,
      poweredOff: true,
      selected: true,
      dynamics: {} as never,
    }),
  ).toEqual({
    'data-bypassed': true,
    'data-offline': true,
    'data-powered-off': true,
    'data-selected': true,
    'data-dynamics': true,
  });
});

describe('card geometry', () => {
  const minimum = audioNodeFaceGeometry({ mode: 'standard' }).height;

  it('never shrinks the face below its geometry and uses the rendered face once measured', () => {
    const style = audioNodeCardStyle({
      faceHeight: 1,
      faceMode: 'standard',
      renderedFaceHeight: 0,
      headerActionsWidth: 40,
    }) as Record<string, string>;
    expect(style['--audio-node-face-height']).toBe(`${minimum}px`);
    expect(style['--audio-node-rendered-face-height']).toBe(`${minimum}px`);
    expect(style['--audio-node-header-height']).toBe(`${audioNodeFrame.headerHeight}px`);
    expect(style['--audio-node-header-actions-inset']).toBe(`${40 + spacing[3]}px`);
  });

  it('keeps a taller requested face and reports the measured face separately', () => {
    const style = audioNodeCardStyle({
      faceHeight: minimum + 100,
      faceMode: 'standard',
      renderedFaceHeight: 77,
      headerActionsWidth: 0,
    }) as Record<string, string>;
    expect(style['--audio-node-face-height']).toBe(`${minimum + 100}px`);
    expect(style['--audio-node-rendered-face-height']).toBe('77px');
  });
});
