import { act, fireEvent, render, waitFor } from '@testing-library/react';
import { assert, describe, expect, it, vi } from 'vitest';

import { AudioMeterProvider } from '../audio-meter/AudioMeterProvider';
import { AudioMeterStore } from '../audio-meter/AudioMeterStore';
import { MixRelayProvider } from '../theme/MixRelayProvider';
import { AudioNodeCard, type AudioNodeCardProps } from './AudioNodeCard';

function card(iconUrl?: string) {
  return (
    <MixRelayProvider>
      <AudioNodeCard
        category="Application"
        icon="APP"
        iconUrl={iconUrl}
        inputCount={0}
        outputCount={2}
        title="Player"
        onSelect={vi.fn()}
      />
    </MixRelayProvider>
  );
}

function installResizeObserverHarness() {
  const original = globalThis.ResizeObserver;
  const callbacks = new Map<Element, ResizeObserverCallback>();
  globalThis.ResizeObserver = class implements ResizeObserver {
    constructor(private readonly callback: ResizeObserverCallback) {}
    observe(target: Element) {
      callbacks.set(target, this.callback);
    }
    unobserve(target: Element) {
      callbacks.delete(target);
    }
    disconnect() {}
  };
  return {
    restore: () => {
      globalThis.ResizeObserver = original;
    },
    emit: (target: Element, height: number, width = 300) => {
      const callback = callbacks.get(target);
      assert(callback);
      act(() =>
        callback(
          [
            {
              target,
              borderBoxSize: [{ blockSize: height, inlineSize: width }],
              contentRect: { x: 0, y: 0, top: 0, left: 0, right: width, bottom: height },
            } as unknown as ResizeObserverEntry,
          ],
          {} as ResizeObserver,
        ),
      );
    },
  };
}

it('reports one DOM-coherent controls/disclosure pair across observer order and reflow', async () => {
  const { emit, restore } = installResizeObserverHarness();
  try {
    const onContentHeightChange = vi.fn();
    const view = render(
      <MixRelayProvider>
        <AudioNodeCard
          category="Input"
          icon="MIC"
          title="Input"
          inputCount={0}
          outputCount={2}
          onSelect={vi.fn()}
          gainControl={{ label: 'Gain', valueDb: 0 }}
          onGainChange={vi.fn()}
          onContentHeightChange={onContentHeightChange}
        />
      </MixRelayProvider>,
    );
    const options = view.getByRole('button', { name: (name) => name.endsWith('Options') });
    const controls = view.container.querySelector<HTMLElement>('[data-node-controls]');
    assert(controls);
    controls.style.boxSizing = 'border-box';
    controls.style.height = '25px';
    emit(controls, 25);
    await waitFor(() => expect(onContentHeightChange).toHaveBeenLastCalledWith(25, 0));

    fireEvent.click(options);
    const panel = view.container.querySelector<HTMLElement>('[data-node-disclosure]');
    assert(panel);
    panel.style.boxSizing = 'border-box';
    panel.style.height = '37.5px';
    controls.style.height = '62.5px';
    emit(panel, 37.5);
    emit(controls, 62.5);
    await waitFor(() => expect(onContentHeightChange).toHaveBeenLastCalledWith(62.5, 37.5));

    onContentHeightChange.mockClear();
    controls.style.height = '85px';
    panel.style.height = '60px';
    emit(controls, 85);
    await waitFor(() => expect(onContentHeightChange).toHaveBeenLastCalledWith(85, 60));
    expect(
      onContentHeightChange.mock.calls.every(([total, disclosure]) => total - disclosure === 25),
    ).toBe(true);
    emit(panel, 60);

    onContentHeightChange.mockClear();
    controls.style.height = '55px';
    panel.style.height = '30px';
    emit(panel, 30);
    await waitFor(() => expect(onContentHeightChange).toHaveBeenLastCalledWith(55, 30));
    expect(
      onContentHeightChange.mock.calls.every(([total, disclosure]) => total - disclosure === 25),
    ).toBe(true);
    emit(controls, 55);

    controls.style.height = '69px';
    panel.style.height = '44px';
    emit(panel, 30, 240);
    await waitFor(() => expect(onContentHeightChange).toHaveBeenLastCalledWith(69, 44));
  } finally {
    restore();
  }
});

it('renders every generic plug-in parameter without an internal scroll viewport', () => {
  const view = render(
    <MixRelayProvider>
      <AudioNodeCard
        category="Processor"
        icon="AU"
        title="Reverb"
        inputCount={2}
        outputCount={2}
        faceMode="compact"
        onSelect={vi.fn()}
        onParameterChange={vi.fn()}
        parameterControls={Array.from({ length: 24 }, (_, index) => ({
          id: `parameter-${index}`,
          label: `Parameter ${index + 1}`,
          minimum: 0,
          maximum: 1,
          step: 0.01,
          suffix: '',
          value: 0.5,
        }))}
      />
    </MixRelayProvider>,
  );
  expect(
    view.queryByRole('button', { name: (name) => name.endsWith('Options') }),
  ).not.toBeInTheDocument();
  expect(view.getAllByRole('slider')).toHaveLength(24);
  expect(view.getByRole('slider', { name: 'Parameter 24' })).toBeVisible();
  expect(view.container.querySelector('[data-radix-scroll-area-viewport]')).toBeNull();
});

it.each([
  {
    status: 'Plug-in stopped',
    detail: 'Saved layout',
    statusTone: 'critical' as const,
    text: 'Plug-in stopped',
  },
  {
    status: undefined,
    detail: 'Unavailable layout',
    statusTone: 'critical' as const,
    text: 'Unavailable layout',
  },
  {
    status: 'Checking',
    detail: 'Last known device',
    statusTone: 'warning' as const,
    text: 'Last known device',
  },
])('keeps compact feedback readable: $text', ({ status, detail, statusTone, text }) => {
  const view = render(
    <MixRelayProvider>
      <AudioNodeCard
        title="Voice effect"
        category="Processor"
        icon="FX"
        inputCount={2}
        outputCount={2}
        faceMode="compact"
        status={status}
        statusTone={statusTone}
        detail={detail}
        content={<span>Recovery action</span>}
      />
    </MixRelayProvider>,
  );
  expect(view.getByText(text)).toBeVisible();
  expect(view.getByText('Recovery action')).toBeVisible();
});

it('places native plug-in content under a compact unmetered face', () => {
  const view = render(
    <MixRelayProvider>
      <AudioNodeCard
        category="Processor"
        icon="AU"
        title="Filter"
        inputCount={2}
        outputCount={2}
        faceMode="compact"
        faceHeight={36}
        onSelect={vi.fn()}
        content={<div>Native editor</div>}
      />
    </MixRelayProvider>,
  );
  const root = view.getByText('Native editor').closest('[data-face-mode]');
  expect(root).toHaveAttribute('data-face-mode', 'compact');
  expect(root).toHaveStyle('--audio-node-face-height: 36px');
  expect(root).not.toHaveAttribute('data-metered');
  expect(view.queryByText('IN 2')).not.toBeInTheDocument();
  expect(view.queryByText('OUT 2')).not.toBeInTheDocument();
});

it('keeps primary effect controls available while advanced values use the shared disclosure', () => {
  const change = vi.fn();
  const view = render(
    <MixRelayProvider>
      <AudioNodeCard
        category="Processor"
        icon="FX"
        title="Reverb"
        faceMode="compact"
        onParameterChange={change}
        inputCount={2}
        outputCount={2}
        parameterControls={[
          {
            id: 'mix',
            label: 'Mix',
            minimum: 0,
            maximum: 1,
            step: 0.01,
            suffix: '',
            value: 0.2,
            precise: true,
            presentation: 'knob',
          },
          {
            id: 'preDelayMs',
            label: 'Pre-delay',
            minimum: 0,
            maximum: 200,
            step: 1,
            suffix: 'ms',
            value: 20,
            precise: true,
            presentation: 'knob',
            section: 'options',
          },
        ]}
      />
    </MixRelayProvider>,
  );
  const options = view.getByRole('button', { name: (name) => name.endsWith('Options') });
  expect(options).toHaveAttribute('aria-expanded', 'false');
  expect(view.getByRole('textbox', { name: 'Mix' })).toBeVisible();
  expect(view.queryByRole('textbox', { name: 'Pre-delay' })).not.toBeInTheDocument();
  fireEvent.click(options);
  expect(options).toHaveAttribute('aria-expanded', 'true');
  const advanced = view.getByRole('textbox', { name: 'Pre-delay' });
  expect(advanced.closest('[data-node-disclosure]')?.id).toBe(
    options.getAttribute('aria-controls'),
  );
  fireEvent.focus(advanced);
  fireEvent.change(advanced, { target: { value: '35' } });
  fireEvent.blur(advanced);
  expect(change).toHaveBeenCalledExactlyOnceWith('preDelayMs', 35);
  fireEvent.click(options);
  expect(view.queryByRole('textbox', { name: 'Pre-delay' })).not.toBeInTheDocument();
  expect(view.getByRole('textbox', { name: 'Mix' })).toBeVisible();
});

it('shows applied dynamics readings instead of duplicate output meters or inferred reduction', () => {
  const store = new AudioMeterStore();
  store.update({
    protocolVersion: 1,
    revision: 1,
    nodes: [
      {
        nodeId: 'compressor',
        portId: 'output',
        peaks: [0.5, 0.25],
        dynamics: { inputPeaks: [0.5, 0.25], gainReductionDb: 8, thresholdDb: -24 },
      },
    ],
  });
  const view = render(
    <MixRelayProvider>
      <AudioMeterProvider store={store}>
        <AudioNodeCard
          category="Processor"
          icon="FX"
          title="Compressor"
          faceMode="compact"
          inputCount={2}
          outputCount={2}
          meterBanks={[{ nodeId: 'compressor', portId: 'output', channels: 2 }]}
          dynamics={{
            nodeId: 'compressor',
            portId: 'output',
            thresholdId: 'thresholdDb',
            inputLabel: 'Input',
            outputLabel: 'Output',
            reductionLabel: 'Gain reduction',
            unavailableLabel: 'Waiting for meter data',
          }}
          onParameterChange={vi.fn()}
          parameterControls={[
            {
              id: 'thresholdDb',
              label: 'Threshold',
              value: -18,
              minimum: -80,
              maximum: 0,
              step: 0.1,
              suffix: 'dB',
              precise: true,
              presentation: 'knob',
            },
          ]}
        />
      </AudioMeterProvider>
    </MixRelayProvider>,
  );
  expect(view.getAllByRole('meter')).toHaveLength(5);
  expect(view.getByRole('meter', { name: 'Gain reduction' })).toHaveAttribute(
    'aria-valuetext',
    '8.0 dB',
  );
  expect(view.getByText('Threshold -24.0 dB')).toBeVisible();
  expect(view.getByRole('textbox', { name: 'Threshold' })).toHaveValue('-18');
  expect(view.queryByRole('meter', { name: 'Compressor channel 1' })).not.toBeInTheDocument();
  act(() => store.clear());
  expect(view.queryByRole('meter')).not.toBeInTheDocument();
  expect(view.queryByText('Waiting for meter data')).not.toBeInTheDocument();
  expect(view.getByRole('img', { name: 'Gain reduction: Waiting for meter data' })).toBeVisible();
});

it.each(['closed', 'opening', 'open', 'failed', 'unavailable', undefined] as const)(
  'prefers the plug-in editor with parameter access when its state is %s',
  (state) => {
    const onToggle = vi.fn();
    const editor: AudioNodeCardProps['processorEditor'] = state
      ? { actionLabel: state === 'closed' ? 'Open editor' : 'Retry editor', state }
      : undefined;
    const view = render(
      <MixRelayProvider>
        <AudioNodeCard
          category="Processor"
          icon="FX"
          inputCount={2}
          outputCount={2}
          title="Filter"
          onSelect={vi.fn()}
          processorEditor={editor}
          onProcessorEditorRetry={onToggle}
          onParameterChange={vi.fn()}
          parameterControls={[
            {
              id: 'cutoff',
              label: 'Cutoff',
              minimum: 0,
              maximum: 1,
              step: 0.01,
              suffix: '',
              value: 0.5,
            },
          ]}
        />
      </MixRelayProvider>,
    );
    const needsParameters = !state || state === 'failed' || state === 'unavailable';
    if (needsParameters) {
      expect(
        view.queryByRole('button', { name: (name) => name.endsWith('Options') }),
      ).not.toBeInTheDocument();
      expect(view.getByRole('slider', { name: 'Cutoff' })).toBeEnabled();
    } else {
      expect(
        view.queryByRole('button', { name: (name) => name.endsWith('Options') }),
      ).not.toBeInTheDocument();
      expect(view.queryByRole('slider')).not.toBeInTheDocument();
    }
    if (state === 'failed' || state === 'closed') {
      const button = view.getByRole('button', {
        name: state === 'closed' ? 'Filter: Open editor' : 'Filter: Retry editor',
      });
      expect(button.textContent).toBe('');
      expect(view.queryByText('Plug-in editor')).not.toBeInTheDocument();
      expect(view.queryByText(state)).not.toBeInTheDocument();
      expect(button.closest('.nodrag.nopan.nowheel')).not.toBeNull();
      fireEvent.click(button);
      expect(onToggle).toHaveBeenCalledOnce();
    } else {
      expect(
        view.queryByRole('button', { name: (name) => name.endsWith('Retry editor') }),
      ).not.toBeInTheDocument();
      expect(
        view.queryByRole('button', { name: (name) => name.endsWith('Open editor') }),
      ).not.toBeInTheDocument();
    }
  },
);

it('renders feature-owned controls without changing the port face boundary', () => {
  const view = render(
    <MixRelayProvider>
      <AudioNodeCard
        category="Output"
        icon="REC"
        title="Recorder"
        inputCount={2}
        outputCount={0}
        faceHeight={160}
        onSelect={vi.fn()}
        content={<button type="button">Record</button>}
      />
    </MixRelayProvider>,
  );
  const control = view.getByRole('button', { name: 'Record' });
  expect(control.closest('.nodrag.nopan.nowheel')).not.toBeNull();
  const cardRoot = control.closest('[data-metered], [style*="--audio-node-face-height"]');
  expect(cardRoot?.getAttribute('style')).toContain('--audio-node-face-height: 160px');
});

describe('application node icon fallback', () => {
  it('uses the application glyph when a supplied icon cannot decode, and retries changed content', () => {
    const view = render(card('data:image/png;base64,broken'));
    const image = view.container.querySelector('img');
    expect(image).not.toBeNull();
    if (!image) throw new Error('Expected native application icon');
    fireEvent.error(image);
    expect(view.container.querySelector('img')).toBeNull();
    expect(view.container.querySelector('svg')).not.toBeNull();
    view.rerender(card('data:image/png;base64,replacement'));
    expect(view.container.querySelector('img')).toHaveAttribute(
      'src',
      'data:image/png;base64,replacement',
    );
  });

  it('keeps fallback glyphs for applications without an icon', () => {
    const view = render(card());
    expect(view.container.querySelector('img')).toBeNull();
    expect(view.container.querySelector('svg')).not.toBeNull();
  });
});

it('clears offline meters while keeping selection and options editable, then restores live levels', () => {
  const onSelect = vi.fn();
  const store = new AudioMeterStore();
  store.update({
    nodes: [{ nodeId: 'microphone', portId: 'audio', peaks: [1, 0] }],
    protocolVersion: 1,
    revision: 1,
  });
  const props = {
    category: 'Input',
    icon: 'MIC',
    title: 'Microphone',
    inputCount: 0,
    outputCount: 2,
    meterBanks: [{ channels: 2, nodeId: 'microphone', portId: 'audio' }],
    onSelect,
    gainControl: { label: 'Gain', valueDb: 0 },
    onGainChange: vi.fn(),
  };
  const view = render(
    <MixRelayProvider>
      <AudioMeterProvider store={store}>
        <AudioNodeCard {...props} offline status="Offline" />
      </AudioMeterProvider>
    </MixRelayProvider>,
  );
  const button = view.getByRole('button', { name: 'Microphone, Input, Offline' });
  expect(button).toBeEnabled();
  expect(button.closest('[data-offline]')).not.toBeNull();
  expect(view.container.querySelectorAll('[title$=": 0%"]')).toHaveLength(2);
  fireEvent.click(button);
  expect(onSelect).toHaveBeenCalledOnce();
  fireEvent.click(view.getByRole('button', { name: (name) => name.endsWith('Options') }));
  expect(view.getByRole('slider', { name: 'Gain' })).toBeEnabled();
  view.rerender(
    <MixRelayProvider>
      <AudioMeterProvider store={store}>
        <AudioNodeCard {...props} />
      </AudioMeterProvider>
    </MixRelayProvider>,
  );
  expect(view.container.querySelector('[data-offline]')).toBeNull();
  expect(view.container.querySelector('[title$=": 100%"]')).not.toBeNull();
  expect(view.container.querySelector('[title$=": 0%"]')).not.toBeNull();
});

it.each([
  { channels: 8, density: undefined },
  { channels: 9, density: 'compact' },
  { channels: 32, density: 'compact' },
  { channels: 64, density: 'dense' },
])(
  'renders an exact $channels-channel meter bank without truncating its channels',
  ({ channels, density }) => {
    const view = render(
      <MixRelayProvider>
        <AudioNodeCard
          category="Processor"
          icon="FX"
          inputCount={channels}
          outputCount={channels}
          meterBanks={[{ channels, nodeId: 'processor', portId: 'audio' }]}
          title={`${channels} channel processor`}
          onSelect={vi.fn()}
        />
      </MixRelayProvider>,
    );
    const meterContainer = view.container.querySelector(`[data-channels="${channels}"]`);
    expect(meterContainer).not.toBeNull();
    if (channels <= 8) {
      expect(meterContainer?.querySelectorAll('[data-orientation="horizontal"]')).toHaveLength(
        channels,
      );
    } else {
      expect(meterContainer).toHaveAttribute('data-density', density);
      expect(meterContainer?.querySelectorAll('[data-orientation="vertical"]')).toHaveLength(
        channels,
      );
    }
  },
);

it('binds provider banks to their authored output IDs and keeps peak levels separate', () => {
  const store = new AudioMeterStore();
  store.update({
    nodes: [
      { nodeId: 'provider', portId: 'audio', peaks: [1, 1] },
      { nodeId: 'provider', portId: 'bus:1', peaks: [0] },
    ],
    protocolVersion: 1,
    revision: 2,
  });
  const view = render(
    <MixRelayProvider>
      <AudioMeterProvider store={store}>
        <AudioNodeCard
          category="Processor"
          icon="VST3"
          inputCount={2}
          outputCount={3}
          faceMode="compact"
          meterBanks={[
            { channels: 2, nodeId: 'provider', portId: 'audio', label: 'Main · Output' },
            { channels: 1, nodeId: 'provider', portId: 'bus:1', label: 'Sidechain · Detector' },
          ]}
          title="Dynamics"
          onSelect={vi.fn()}
        />
      </AudioMeterProvider>
    </MixRelayProvider>,
  );
  expect(view.getByText('Main · Output')).toBeVisible();
  expect(view.getByText('Sidechain · Detector')).toBeVisible();
  expect(
    view.container.querySelector('[title="Dynamics Main · Output channel 1: 100%"]'),
  ).toBeTruthy();
  expect(
    view.container.querySelector('[title="Dynamics Sidechain · Detector channel 1: 0%"]'),
  ).toBeTruthy();
});

it('keeps both directions of a duplex endpoint on their own visible meter banks', () => {
  const store = new AudioMeterStore();
  store.update({
    nodes: [
      { nodeId: 'duplex', portId: 'from_os', peaks: [1, 0] },
      { nodeId: 'duplex', portId: 'to_os', peaks: [0, 1] },
    ],
    protocolVersion: 1,
    revision: 3,
  });
  const view = render(
    <MixRelayProvider>
      <AudioMeterProvider store={store}>
        <AudioNodeCard
          category="Virtual input"
          icon="IN"
          inputCount={0}
          outputCount={2}
          meterBanks={[{ channels: 2, nodeId: 'duplex', portId: 'from_os' }]}
          title="Duplex"
          onSelect={vi.fn()}
        />
        <AudioNodeCard
          category="Virtual output"
          icon="OUT"
          inputCount={2}
          outputCount={0}
          meterBanks={[{ channels: 2, nodeId: 'duplex', portId: 'to_os' }]}
          title="Duplex"
          onSelect={vi.fn()}
        />
      </AudioMeterProvider>
    </MixRelayProvider>,
  );
  const source = view.container.querySelector('[data-meter-port="from_os"]');
  const sink = view.container.querySelector('[data-meter-port="to_os"]');
  expect(source?.querySelectorAll('[data-orientation="horizontal"]')).toHaveLength(2);
  expect(sink?.querySelectorAll('[data-orientation="horizontal"]')).toHaveLength(2);
  expect(source?.querySelectorAll('[title$=": 100%"]')).toHaveLength(1);
  expect(sink?.querySelectorAll('[title$=": 100%"]')).toHaveLength(1);
});

it('renders a display face when the parent owns selection, while options stay interactive', () => {
  const view = render(
    <MixRelayProvider>
      <AudioNodeCard
        category="Application"
        icon="APP"
        title="Player"
        inputCount={0}
        outputCount={2}
        gainControl={{ label: 'Gain', valueDb: 0 }}
        onGainChange={vi.fn()}
      />
    </MixRelayProvider>,
  );
  expect(view.queryByRole('button', { name: 'Player, Application' })).not.toBeInTheDocument();
  expect(view.container.querySelector('[data-node-face]')?.tagName).toBe('DIV');
  fireEvent.click(view.getByRole('button', { name: (name) => name.endsWith('Options') }));
  expect(view.getByRole('slider', { name: 'Gain' })).toBeInTheDocument();
});

it('includes content-box padding and borders and rejects incomplete DOM measurements', () => {
  const { emit, restore } = installResizeObserverHarness();
  try {
    const onContentHeightChange = vi.fn();
    const onFaceHeightChange = vi.fn();
    const view = render(
      <MixRelayProvider>
        <AudioNodeCard
          title="Input"
          category="Input"
          icon="MIC"
          inputCount={0}
          outputCount={2}
          gainControl={{ label: 'Gain', valueDb: 0 }}
          onGainChange={vi.fn()}
          onContentHeightChange={onContentHeightChange}
          onFaceHeightChange={onFaceHeightChange}
        />
      </MixRelayProvider>,
    );
    const controls = view.container.querySelector<HTMLElement>('[data-node-controls]');
    const face = view.container.querySelector<HTMLElement>('[data-node-face]');
    assert(controls && face);
    Object.assign(controls.style, {
      boxSizing: 'content-box',
      height: '20px',
      paddingTop: '2px',
      paddingBottom: '3px',
      borderTopWidth: '1px',
      borderBottomWidth: '1px',
    });
    emit(controls, 27);
    expect(onContentHeightChange).toHaveBeenLastCalledWith(27, 0);
    emit(face, 64);
    expect(onFaceHeightChange).toHaveBeenLastCalledWith(64);
    controls.style.paddingTop = 'var(--pending-layout)';
    controls.style.paddingBottom = 'var(--pending-layout)';
    controls.style.borderTopWidth = 'var(--pending-layout)';
    controls.style.borderBottomWidth = 'var(--pending-layout)';
    emit(controls, 20);
    expect(onContentHeightChange).toHaveBeenLastCalledWith(20, 0);
    onContentHeightChange.mockClear();
    controls.style.height = 'auto';
    emit(controls, 30);
    expect(onContentHeightChange).not.toHaveBeenCalled();
    controls.style.height = '40px';
    fireEvent.click(view.getByRole('button', { name: (name) => name.endsWith('Options') }));
    const panel = view.container.querySelector<HTMLElement>('[data-node-disclosure]');
    assert(panel);
    onContentHeightChange.mockClear();
    panel.style.height = 'auto';
    emit(controls, 41);
    expect(onContentHeightChange).not.toHaveBeenCalled();
    panel.style.height = '80px';
    emit(controls, 42);
    expect(onContentHeightChange).not.toHaveBeenCalled();
  } finally {
    restore();
  }
});

describe('channel layout choice', () => {
  const control = {
    label: 'Channel layout',
    value: 'stereo',
    options: [
      { value: 'mono', label: 'Mono' },
      { value: 'stereo', label: 'Stereo' },
    ],
  };
  const props = {
    category: 'Processor',
    icon: 'AU',
    inputCount: 2,
    outputCount: 2,
    title: 'AUBandpass',
  };

  it.each([
    ['without parameters', []],
    [
      'beside inline parameters',
      [{ id: 'gain', label: 'Gain', minimum: 0, maximum: 1, step: 0.1, suffix: '', value: 0.5 }],
    ],
  ])('is offered in Options %s and reports the choice', (_case, parameterControls) => {
    const onChannelLayoutChange = vi.fn();
    const view = render(
      <MixRelayProvider>
        <AudioNodeCard
          {...props}
          parameterControls={parameterControls}
          onParameterChange={vi.fn()}
          channelLayoutControl={control}
          onChannelLayoutChange={onChannelLayoutChange}
        />
      </MixRelayProvider>,
    );
    fireEvent.click(view.getByRole('button', { name: 'AUBandpass: Options' }));
    fireEvent.click(view.getByRole('button', { name: /Channel layout$/ }));
    fireEvent.click(view.getByRole('option', { name: 'Mono' }));
    expect(onChannelLayoutChange).toHaveBeenCalledWith('mono');
  });

  it('is not offered with a single layout or without an owner', () => {
    const single = {
      ...control,
      options: [control.options[1] as (typeof control.options)[number]],
    };
    const view = render(
      <MixRelayProvider>
        <AudioNodeCard {...props} channelLayoutControl={single} onChannelLayoutChange={vi.fn()} />
        <AudioNodeCard {...props} title="Other" channelLayoutControl={control} />
      </MixRelayProvider>,
    );
    expect(view.queryByRole('button', { name: /Options$/ })).toBeNull();
  });
});
