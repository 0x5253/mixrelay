import { StorybookMockControlPort } from '../../core/adapters/fake/StorybookMockControlPort';
import type { RuntimeGraphDocument } from '../../core/domain/control/graph';
import '../../test-support/integration';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { AudioGraphCanvasProps } from '../../core/audio-graph/AudioGraphCanvas';
import { I18nProvider, i18n } from '../../core/i18n';
import { MixRelayProvider } from '../../core/theme/MixRelayProvider';
import { AudioNodeCard } from '../../core/ui/AudioNodeCard';
import { HierarchicalMenuButton } from '../../core/ui/HierarchicalMenu';
import {
  advance,
  eventually,
  find,
  findIn,
  finishFakeTimerTest,
} from '../../test-support/fakeTimerQueries';
import { effectConfig } from './builtinEffects';
import { updateRoutingNodeParameter } from './model';
import { RoutingWorkspace } from './RoutingWorkspace';

const harness = vi.hoisted(() => ({ canvas: null as AudioGraphCanvasProps | null }));

// Substitute only the spatial canvas: keep the real Core menu and parameter controls,
// and exercise RoutingWorkspace's selection, command application and history owners.
vi.mock('../../core/audio-graph/AudioGraphCanvas', () => ({
  AudioGraphCanvas: (props: AudioGraphCanvasProps) => {
    harness.canvas = props;
    return (
      <section aria-label={props.ariaLabel}>
        <HierarchicalMenuButton
          label={props.addNodeLabel ?? 'Add node'}
          buttonContent={props.addNodeLabel}
          items={props.nodeMenuItems ?? []}
          onOpenedChange={props.onNodeMenuOpenedChange}
          onSelect={(id) => props.onAddNode?.(id, { x: 44, y: 33 })}
        />
        {props.model.nodes.map((node) => {
          const controls = props.renderNodeControls?.(node);
          return (
            <section key={node.id} aria-label={node.appearance.title}>
              <AudioNodeCard
                {...node.appearance}
                optionsLabel={node.appearance.optionsLabel ?? i18n.t('routing.nodes.options')}
                headerActions={controls?.power}
                muteControl={controls?.mute}
                poweredOff={node.runtime?.control?.enabled === false}
                faceMode="compact"
                inputCount={node.inputPorts.length}
                outputCount={node.outputPorts.length}
                onParameterChange={
                  props.onNodeParameterChange
                    ? (id, value) => props.onNodeParameterChange?.(node.id, id, value)
                    : undefined
                }
                onParametersChange={
                  props.onNodeParametersChange
                    ? (updates) => props.onNodeParametersChange?.(node.id, updates)
                    : undefined
                }
              />
            </section>
          );
        })}
      </section>
    );
  },
}));

const empty: RuntimeGraphDocument = { schemaVersion: 2, revision: 1, nodes: [], edges: [] };

function createPort(graph = empty) {
  return new StorybookMockControlPort({
    graph,
    graphStatus: {
      protocolVersion: 1,
      activeRevision: graph.revision,
      submittedRevision: graph.revision,
    },
    status: {
      protocolVersion: 1,
      driverState: 'ready',
      sampleRate: 48000,
      bufferFrames: 128,
      underruns: 0,
      overruns: 0,
    },
    processors: { protocolVersion: 1, processors: [] },
  });
}

function workspace(
  port: StorybookMockControlPort,
  graphId?: string,
  sessionDocument?: RuntimeGraphDocument,
) {
  return (
    <MixRelayProvider>
      <I18nProvider>
        <RoutingWorkspace controlPort={port} graphId={graphId} sessionDocument={sessionDocument} />
      </I18nProvider>
    </MixRelayProvider>
  );
}

function fixture(graph = empty, graphId?: string) {
  const port = createPort(graph);
  const apply = vi.spyOn(port, 'applyGraph');
  const parameter = vi.spyOn(port, 'setProcessorParameter');
  const view = render(workspace(port, graphId));
  return { port, apply, parameter, view };
}

async function ready() {
  // The first render of a whole screen can exceed waitFor's 1 s default under load.
  await find('region', { name: 'MixRelay virtual I/O routing graph' }, { timeout: 5000 });
  await advance();
}

async function addFromMenu(name: string, chain = false) {
  fireEvent.click(screen.getByRole('button', { name: 'Add node' }));
  for (const label of chain ? ['Processor', 'Effect chains'] : ['Processor']) {
    const item = await find('menuitem', { name: label });
    act(() => item.focus());
    fireEvent.keyDown(item, { key: 'ArrowRight' });
  }
  fireEvent.click(await find('menuitem', { name }));
  await eventually(() => expect(screen.queryAllByRole('menu')).toHaveLength(0));
  await advance();
}

async function choose(label: string, option: string) {
  const trigger = screen.getByRole('button', { name: new RegExp(`${label}$`) });
  act(() => trigger.focus());
  fireEvent.click(trigger);
  fireEvent.click(await screen.findByRole('option', { name: option }));
  await act(async () => Promise.resolve());
}

function openEqualizerOptions(label = i18n.t('routing.nodes.options'), nodeName = 'Equalizer') {
  const options = within(screen.getByRole('region', { name: nodeName })).getByRole('button', {
    name: `${nodeName}: ${label}`,
  });
  expect(options).toHaveAttribute('aria-expanded', 'false');
  fireEvent.click(options);
  expect(options).toHaveAttribute('aria-expanded', 'true');
  return options;
}

function lastDocument(apply: ReturnType<typeof fixture>['apply']) {
  const document = apply.mock.calls.at(-1)?.[0];
  if (!document) throw new Error('No graph command was submitted');
  return document;
}

function configOf(
  node: RuntimeGraphDocument['nodes'][number] | undefined,
): Record<string, unknown> {
  const config = node?.config;
  if (!config || typeof config !== 'object' || Array.isArray(config))
    throw new Error('Expected an object config for the created audio node');
  return config as Record<string, unknown>;
}

async function history(redo = false) {
  act(() => screen.getByRole('button', { name: 'Add node' }).focus());
  fireEvent.keyDown(window, { key: 'z', ctrlKey: true, shiftKey: redo });
  await act(async () => Promise.resolve());
}

beforeEach(async () => {
  harness.canvas = null;
  localStorage.clear();
  await i18n.changeLanguage('en');
  // jsdom has no layout; these measurements let React Aria place its real overlays.
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue(
    new DOMRect(20, 20, 240, 32),
  );
  vi.spyOn(document.documentElement, 'clientWidth', 'get').mockReturnValue(1024);
  vi.spyOn(document.documentElement, 'clientHeight', 'get').mockReturnValue(768);
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it.each([
  ['parameter', 'loading'],
  ['preset', 'loading'],
  ['add', 'loading'],
  ['parameter', 'active'],
  ['preset', 'active'],
  ['add', 'active'],
] as const)(
  'discards a previous canvas %s callback when the replacement engine is %s',
  async (action, state) => {
    const original: RuntimeGraphDocument = {
      ...empty,
      nodes: [
        {
          id: 'old',
          label: 'Old reverb',
          type: 'effect',
          config: effectConfig('reverb'),
        },
      ],
    };
    const { apply, view } = fixture(original);
    await ready();
    const stale = harness.canvas;
    if (!stale) throw new Error('Original canvas is not ready');
    const parameterId = action === 'preset' ? 'preset' : 'decaySeconds';
    const value = action === 'preset' ? 3 : 2.3;
    if (action !== 'add') {
      // The stale callback must target a live, editable control for its discard to matter.
      const control = stale.model.nodes[0]?.appearance.parameterControls?.find(
        (candidate) => candidate.id === parameterId,
      );
      expect(control?.value).toEqual(expect.any(Number));
      expect(control?.value).not.toBe(value);
      expect(updateRoutingNodeParameter(stale.model, 'old', parameterId, value)).not.toBe(
        stale.model,
      );
    }
    const replacement = createPort();
    let resolveGraph: ((document: RuntimeGraphDocument) => void) | undefined;
    vi.spyOn(replacement, 'getGraph').mockReturnValue(
      new Promise((resolve) => {
        resolveGraph = resolve;
      }),
    );
    const replacementApply = vi.spyOn(replacement, 'applyGraph');
    view.rerender(workspace(replacement));
    await act(async () => Promise.resolve());
    expect(screen.queryByRole('region', { name: 'Old reverb' })).toBeNull();
    if (state === 'active') {
      await act(async () => resolveGraph?.(empty));
      await ready();
    }
    // A deferred drag/number commit can retain the previous Core control's callback.
    act(() => {
      if (action === 'add') stale.onAddNode?.('builtin:equalizer', { x: 44, y: 33 });
      else stale.onNodeParameterChange?.('old', parameterId, value);
    });
    await act(async () => Promise.resolve());
    expect(apply).not.toHaveBeenCalled();
    expect(replacementApply).not.toHaveBeenCalled();
    expect(screen.queryByRole('region', { name: 'Old reverb' })).toBeNull();
    await act(async () => resolveGraph?.(empty));
    await ready();
    expect(harness.canvas?.model.nodes).toEqual([]);
    await history();
    expect(replacementApply).not.toHaveBeenCalled();
    view.unmount();
  },
);

it.each(['parameter', 'preset', 'add'] as const)(
  'discards a late %s callback after the workspace unmounts',
  async (action) => {
    const { apply, parameter, view } = fixture();
    await ready();
    await addFromMenu('Reverb');
    await waitFor(() => expect(apply).toHaveBeenCalledTimes(1));
    const stale = harness.canvas;
    const nodeId = stale?.model.nodes[0]?.id;
    if (!stale || !nodeId) throw new Error('Original reverb is not ready');
    apply.mockClear();
    view.unmount();
    act(() => {
      if (action === 'add') stale.onAddNode?.('builtin:equalizer', { x: 44, y: 33 });
      else
        stale.onNodeParameterChange?.(
          nodeId,
          action === 'preset' ? 'preset' : 'decaySeconds',
          action === 'preset' ? 3 : 2.3,
        );
    });
    await act(async () => Promise.resolve());
    expect(apply).not.toHaveBeenCalled();
    expect(parameter).not.toHaveBeenCalled();
    expect(screen.queryByRole('region', { name: 'Reverb' })).toBeNull();
  },
);

it('discards the previous graph owner knob callback while allowing the current graph to edit', async () => {
  const { port, apply, view } = fixture(empty, 'graph-1');
  await ready();
  await addFromMenu('Reverb');
  await waitFor(() => expect(apply).toHaveBeenCalledTimes(1));
  const stale = harness.canvas;
  const nodeId = stale?.model.nodes[0]?.id;
  if (!stale || !nodeId) throw new Error('Original reverb is not ready');
  const catalog = await port.getGraphCatalog();
  const replacement = await port.mutateGraphCatalog({
    action: 'create',
    name: 'Replacement',
    expectedRevision: catalog.document.revision,
    expectedCatalogRevision: catalog.catalogRevision,
  });
  const layout = vi.spyOn(port, 'setGraphLayout');
  view.rerender(workspace(port, replacement.activeId, replacement.document));
  await ready();
  apply.mockClear();
  layout.mockClear();
  act(() => stale.onNodeParameterChange?.(nodeId, 'decaySeconds', 2.3));
  await act(async () => Promise.resolve());
  expect(screen.queryByRole('region', { name: 'Reverb' })).toBeNull();
  expect(harness.canvas?.model.nodes).toEqual([]);
  expect(apply).not.toHaveBeenCalled();
  expect(layout).not.toHaveBeenCalled();
  await history();
  expect(apply).not.toHaveBeenCalled();
  await addFromMenu('Reverb');
  await waitFor(() => expect(apply).toHaveBeenCalledTimes(1));
  fireEvent.keyDown(screen.getByRole('slider', { name: 'Decay' }), { key: 'ArrowUp' });
  await waitFor(() => expect(apply).toHaveBeenCalledTimes(2));
  expect(lastDocument(apply)).toMatchObject({
    nodes: [
      expect.objectContaining({
        config: expect.objectContaining({
          effect: expect.objectContaining({
            parameters: expect.objectContaining({ decaySeconds: 1.3 }),
          }),
        }),
      }),
    ],
  });
  expect(await port.getGraphCatalog()).toMatchObject({
    activeId: replacement.activeId,
    document: { nodes: lastDocument(apply).nodes },
  });
  view.unmount();
});

it('uses translated effect names and parameter labels after changing the workspace language', async () => {
  const { apply, view } = fixture();
  await ready();
  await act(async () => i18n.changeLanguage('ko-KR'));
  fireEvent.click(screen.getByRole('button', { name: i18n.t('routing.addNode') }));
  const processor = await screen.findByRole('menuitem', {
    name: i18n.t('routing.groups.processor'),
  });
  act(() => processor.focus());
  fireEvent.keyDown(processor, { key: 'ArrowRight' });
  fireEvent.click(await screen.findByRole('menuitem', { name: '이퀄라이저' }));
  await waitFor(() => expect(apply).toHaveBeenCalledTimes(1));
  expect(lastDocument(apply).nodes[0]).toMatchObject({
    label: '이퀄라이저',
    config: { effect: { kind: 'equalizer' } },
  });
  openEqualizerOptions(i18n.t('routing.nodes.options'), '이퀄라이저');
  expect(screen.getByRole('textbox', { name: '1 저음 Hz' })).toHaveValue('150');
  expect(screen.getByRole('textbox', { name: '2 중음 Q' })).toHaveValue('0.707');
  view.unmount();
});

// Every mode's name and description is unit-tested; this covers the choice control's
// focus after the graph applies, and Undo/Redo of that single choice.
it('keeps focus on the channel mode choice after it applies, and undoes only that choice', async () => {
  const { apply, view } = fixture();
  await ready();
  await addFromMenu('Channel utility');
  const original = lastDocument(apply).nodes;
  await choose('Channel mode', 'Duplicate left');
  const select = screen.getByRole('button', { name: /Channel mode$/ });
  expect(select).toHaveAccessibleDescription('Send the left input to both outputs (L → L/R).');
  await waitFor(() => expect(select).toHaveFocus());
  const selected = lastDocument(apply).nodes;
  expect(selected[0]?.config).toMatchObject({ effect: { parameters: { mode: 2 } } });
  await history();
  expect(lastDocument(apply).nodes).toEqual(original);
  await history(true);
  expect(lastDocument(apply).nodes).toEqual(selected);
  view.unmount();
});

// Chain contents, wiring and spacing are unit-tested in builtinEffects.test.ts; this
// covers the real submenu and one-step Undo/Redo of the whole insertion.
it.each([['Karaoke', ['delay', 'reverb']]] as const)(
  'inserts %s with connected stereo channels and undoes the entire chain in one step',
  async (name, kinds) => {
    const existing: RuntimeGraphDocument = {
      ...empty,
      nodes: [
        { id: 'existing', label: 'Existing limiter', type: 'limiter', config: { channels: 2 } },
      ],
    };
    const { apply, parameter, view } = fixture(existing);
    await ready();
    await addFromMenu(name, true);
    await waitFor(() => expect(apply).toHaveBeenCalledTimes(1));
    const inserted = lastDocument(apply);
    expect(inserted.nodes[0]).toEqual(existing.nodes[0]);
    const chain = inserted.nodes.slice(1);
    expect(
      chain.map((node) =>
        node.type === 'effect'
          ? (node.config as { effect: { kind: string } }).effect.kind
          : node.type,
      ),
    ).toEqual(kinds);
    expect(inserted.edges).toHaveLength((kinds.length - 1) * 2);
    await history();
    await waitFor(() => expect(lastDocument(apply).nodes).toEqual(existing.nodes));
    expect(lastDocument(apply).edges).toEqual([]);
    await history(true);
    await waitFor(() => expect(lastDocument(apply).nodes).toEqual(inserted.nodes));
    expect(lastDocument(apply).edges).toEqual(inserted.edges);
    expect(parameter).not.toHaveBeenCalled();
    view.unmount();
  },
);

// Each EQ value's isolation is unit-tested; this covers the shared Options disclosure.
it.each([['1 Low Hz', 'lowFrequencyHz', 350]] as const)(
  'edits %s directly after opening shared node options and preserves the other eight values',
  async (label, id, value) => {
    const { apply, view } = fixture();
    await ready();
    await addFromMenu('Equalizer');
    const original = lastDocument(apply).nodes;
    expect(screen.queryByRole('textbox', { name: label })).not.toBeInTheDocument();
    const options = openEqualizerOptions();
    const input = screen.getByRole('textbox', { name: label });
    act(() => input.focus());
    fireEvent.change(input, { target: { value: String(value) } });
    fireEvent.blur(input);
    await act(async () => Promise.resolve());
    const originalConfig = original[0]?.config as {
      effect: { parameters: Record<string, number> };
    };
    expect(lastDocument(apply).nodes[0]?.config).toMatchObject({
      effect: { parameters: { ...originalConfig.effect.parameters, [id]: value } },
    });
    expect(screen.getByRole('textbox', { name: label })).toHaveValue(String(value));
    fireEvent.click(options);
    expect(options).toHaveAttribute('aria-expanded', 'false');
    expect(screen.queryByRole('textbox', { name: label })).not.toBeInTheDocument();
    await history();
    expect(lastDocument(apply).nodes).toEqual(original);
    view.unmount();
  },
);

it('adjusts an EQ number with drag, wheel and keyboard while keeping the selected band and Undo coherent', async () => {
  const { apply, view } = fixture();
  await ready();
  await addFromMenu('Equalizer');
  const original = lastDocument(apply).nodes;
  openEqualizerOptions();
  const input = screen.getByRole('textbox', { name: '3 High Q' });
  const surface = input.closest<HTMLDivElement>('.nowheel');
  if (!surface) throw new Error('The direct number control has no pointer surface');
  surface.setPointerCapture = vi.fn();
  surface.hasPointerCapture = vi.fn(() => true);
  surface.releasePointerCapture = vi.fn();
  fireEvent.pointerDown(input, { pointerId: 73, button: 0, clientY: 100 });
  fireEvent.pointerMove(surface, { pointerId: 73, clientY: 95 });
  fireEvent.pointerUp(surface, { pointerId: 73 });
  await act(async () => Promise.resolve());
  expect(screen.getByRole('textbox', { name: '3 High Q' })).toHaveValue('0.712');
  expect(screen.getByRole('button', { name: '3 High' })).toHaveAttribute('aria-pressed', 'true');
  const dragged = lastDocument(apply).nodes;
  await history();
  expect(lastDocument(apply).nodes).toEqual(original);
  await history(true);
  expect(lastDocument(apply).nodes).toEqual(dragged);
  fireEvent.wheel(surface, { deltaY: -100 });
  await act(async () => Promise.resolve());
  expect(screen.getByRole('textbox', { name: '3 High Q' })).toHaveValue('0.713');
  const wheeled = lastDocument(apply).nodes;
  await history();
  expect(lastDocument(apply).nodes).toEqual(dragged);
  await history(true);
  expect(lastDocument(apply).nodes).toEqual(wheeled);
  fireEvent.keyDown(input, { key: 'ArrowUp' });
  await act(async () => Promise.resolve());
  expect(screen.getByRole('textbox', { name: '3 High Q' })).toHaveValue('0.714');
  expect(surface.releasePointerCapture).toHaveBeenCalledWith(73);
  const changed = lastDocument(apply).nodes;
  await history();
  expect(lastDocument(apply).nodes).toEqual(wheeled);
  await history(true);
  expect(lastDocument(apply).nodes).toEqual(changed);
  view.unmount();
});

it('keeps every EQ wheel step arriving before React renders the next value', async () => {
  const { apply, view } = fixture();
  await ready();
  await addFromMenu('Equalizer');
  const original = lastDocument(apply).nodes;
  openEqualizerOptions();
  const input = screen.getByRole('textbox', { name: '2 Mid Q' });
  const surface = input.closest<HTMLDivElement>('.nowheel');
  if (!surface) throw new Error('The direct number control has no wheel surface');
  act(() => {
    fireEvent.wheel(surface, { deltaY: -100 });
    fireEvent.wheel(surface, { deltaY: -100 });
    fireEvent.wheel(surface, { deltaY: -100 });
  });
  await act(async () => Promise.resolve());
  expect(screen.getByRole('textbox', { name: '2 Mid Q' })).toHaveValue('0.71');
  expect(lastDocument(apply).nodes[0]?.config).toMatchObject({
    effect: { parameters: { midQ: 0.71 } },
  });
  await history();
  expect(lastDocument(apply).nodes).toEqual(original);
  view.unmount();
});

it('moves EQ frequency and gain together and restores both with one Undo', async () => {
  vi.useFakeTimers();
  try {
    const { apply, view } = fixture();
    await ready();
    await addFromMenu('Equalizer');
    expect(screen.queryByRole('textbox', { name: '2 Mid Hz' })).not.toBeInTheDocument();
    const original = lastDocument(apply).nodes;
    const band = screen.getByRole('button', { name: /^2 Mid,/ });
    const plot = band.parentElement;
    if (!plot) throw new Error('The EQ graph is missing its pointer surface');
    vi.spyOn(plot, 'getBoundingClientRect').mockReturnValue(new DOMRect(0, 0, 300, 120));
    plot.setPointerCapture = vi.fn();
    plot.hasPointerCapture = vi.fn(() => true);
    plot.releasePointerCapture = vi.fn();
    fireEvent.pointerDown(band, { pointerId: 71, button: 0, clientX: 178, clientY: 53 });
    fireEvent.pointerMove(plot, { pointerId: 71, clientX: 220, clientY: 30 });
    // Keep both moves inside the 400 ms idle window, independent of runner speed.
    await advance(100);
    fireEvent.pointerMove(plot, { pointerId: 71, clientX: 230, clientY: 24 });
    await act(async () => Promise.resolve());
    fireEvent.pointerUp(plot, { pointerId: 71 });
    expect(plot.setPointerCapture).toHaveBeenCalledWith(71);
    expect(plot.releasePointerCapture).toHaveBeenCalledWith(71);
    const config = lastDocument(apply).nodes[0]?.config as {
      effect: { parameters: Record<string, number> };
    };
    expect(config.effect.parameters.midFrequencyHz).toBeGreaterThan(1000);
    expect(config.effect.parameters.midDb).toBeGreaterThan(0);
    const originalConfig = original[0]?.config as {
      effect: { parameters: Record<string, number> };
    };
    expect(config.effect.parameters.midQ).toBe(originalConfig.effect.parameters.midQ);
    expect(config.effect.parameters.lowFrequencyHz).toBe(150);
    expect(config.effect.parameters.highFrequencyHz).toBe(6000);
    const moved = lastDocument(apply).nodes;
    await history();
    expect(lastDocument(apply).nodes).toEqual(original);
    await history(true);
    expect(lastDocument(apply).nodes).toEqual(moved);
    view.unmount();
  } finally {
    await finishFakeTimerTest(() => act(async () => vi.runOnlyPendingTimersAsync()));
  }
});

it('compares EQ bypass in one click without changing its settings or node signal controls', async () => {
  const { apply, port, view } = fixture(empty, 'graph-1');
  const control = vi.spyOn(port, 'updateNodeControl');
  await ready();
  await addFromMenu('Equalizer');
  const initial = lastDocument(apply).nodes[0];
  if (!initial) throw new Error('The EQ node was not created');
  const button = screen.getByRole('button', {
    name: (name) => name.endsWith('Bypass: Processing'),
  });
  expect(button).toHaveAttribute('aria-pressed', 'false');
  fireEvent.click(button);
  await act(async () => Promise.resolve());
  expect(
    screen.getByRole('button', { name: (name) => name.endsWith('Bypass: Bypassed') }),
  ).toHaveAttribute('aria-pressed', 'true');
  expect(lastDocument(apply).nodes[0]).toMatchObject({
    config: { bypass: true, effect: (initial.config as { effect: unknown }).effect },
  });
  expect(lastDocument(apply).nodes[0]?.control).toEqual(initial?.control);
  expect(control).not.toHaveBeenCalled();
  fireEvent.click(
    screen.getByRole('button', { name: (name) => name.endsWith('Bypass: Bypassed') }),
  );
  await act(async () => Promise.resolve());
  expect(lastDocument(apply).nodes[0]).toEqual(initial);
  view.unmount();
});

it.each(['Compressor', 'Limiter'] as const)(
  'uses only Bypass in a normally processing %s header and preserves its DSP settings',
  async (name) => {
    const { port, apply, view } = fixture();
    const control = vi.spyOn(port, 'updateNodeControl');
    await ready();
    await addFromMenu(name);
    const original = lastDocument(apply).nodes[0];
    const card = within(screen.getByRole('region', { name }));
    for (const action of ['Disable', 'Mute', 'Enable', 'Unmute'])
      expect(card.queryByRole('button', { name: `${action} ${name}` })).not.toBeInTheDocument();
    expect(card.getAllByRole('button', { name: /Bypass:/ })).toHaveLength(1);
    fireEvent.click(
      card.getByRole('button', { name: (name) => name.endsWith('Bypass: Processing') }),
    );
    await act(async () => Promise.resolve());
    expect(lastDocument(apply).nodes[0]).toEqual({
      ...original,
      config: { ...configOf(original), bypass: true },
    });
    expect(control).not.toHaveBeenCalled();
    fireEvent.click(
      card.getByRole('button', { name: (name) => name.endsWith('Bypass: Bypassed') }),
    );
    await act(async () => Promise.resolve());
    expect(lastDocument(apply).nodes[0]).toEqual(original);
    view.unmount();
  },
);

it.each(['Compressor', 'Limiter'] as const)(
  'recovers stored Off and Mute on %s separately while retaining its bypass and DSP settings',
  async (name) => {
    vi.useFakeTimers();
    const created = fixture();
    await ready();
    await addFromMenu(name);
    const initial = lastDocument(created.apply);
    const graph: RuntimeGraphDocument = {
      ...initial,
      nodes: initial.nodes.map((node) => ({
        ...node,
        config: { ...configOf(node), bypass: true, releaseMs: 177 },
        control: { enabled: false, muted: true },
      })),
    };
    created.view.unmount();
    const { port, view } = fixture(graph, 'graph-1');
    const control = vi.spyOn(port, 'updateNodeControl');
    await ready();
    const card = within(screen.getByRole('region', { name }));
    expect(card.queryByRole('button', { name: `Disable ${name}` })).not.toBeInTheDocument();
    expect(card.queryByRole('button', { name: `Mute ${name}` })).not.toBeInTheDocument();
    expect(
      card.getByRole('button', { name: (name) => name.endsWith('Bypass: Bypassed') }),
    ).toBeDisabled();
    const enable = await findIn(card, 'button', { name: `Enable ${name}` });
    act(() => enable.focus());
    fireEvent.click(enable);
    await eventually(() =>
      expect(card.queryByRole('button', { name: `Enable ${name}` })).not.toBeInTheDocument(),
    );
    expect(control).toHaveBeenLastCalledWith(
      'graph-1',
      graph.nodes[0]?.id,
      expect.objectContaining({ enabled: true }),
    );
    expect((await port.getGraph()).nodes[0]?.control).toEqual({ enabled: true, muted: true });
    expect((await port.getGraph()).nodes[0]?.config).toEqual(graph.nodes[0]?.config);
    const unmute = card.getByRole('button', { name: `Unmute ${name}` });
    expect(unmute).toHaveFocus();
    fireEvent.click(unmute);
    await eventually(() =>
      expect(card.queryByRole('button', { name: `Unmute ${name}` })).not.toBeInTheDocument(),
    );
    expect(control).toHaveBeenLastCalledWith(
      'graph-1',
      graph.nodes[0]?.id,
      expect.objectContaining({ muted: false }),
    );
    const restored = await port.getGraph();
    expect(restored.nodes[0]?.control).toEqual({ enabled: true, muted: false });
    expect(restored.nodes[0]?.config).toEqual(graph.nodes[0]?.config);
    expect(
      card.getByRole('button', { name: (name) => name.endsWith('Bypass: Bypassed') }),
    ).toBeEnabled();
    expect(
      card.getByRole('button', { name: (name) => name.endsWith('Bypass: Bypassed') }),
    ).toHaveFocus();
    expect(card.queryByRole('button', { name: `Disable ${name}` })).not.toBeInTheDocument();
    expect(card.queryByRole('button', { name: `Mute ${name}` })).not.toBeInTheDocument();
    view.unmount();
  },
);

it('loads externally disabled EQ settings and offers only recovery to processing', async () => {
  vi.useFakeTimers();
  const created = fixture();
  await ready();
  await addFromMenu('Equalizer');
  const original = lastDocument(created.apply);
  const disabledGraph: RuntimeGraphDocument = {
    ...original,
    nodes: original.nodes.map((node) => ({
      ...node,
      control: { enabled: false, muted: false },
    })),
  };
  created.view.unmount();
  const { apply, port, view } = fixture(disabledGraph, 'graph-1');
  const control = vi.spyOn(port, 'updateNodeControl');
  await ready();
  openEqualizerOptions();
  expect(screen.queryByRole('button', { name: 'Disable Equalizer' })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Mute Equalizer' })).not.toBeInTheDocument();
  expect(harness.canvas?.model.nodes[0]?.runtime?.control?.enabled).toBe(false);
  expect(
    screen.getByRole('button', { name: (name) => name.endsWith('Bypass: Processing') }),
  ).toBeDisabled();
  for (const input of within(screen.getByRole('region', { name: 'Equalizer' })).getAllByRole(
    'textbox',
  ))
    expect(input).toBeDisabled();
  const count = apply.mock.calls.length;
  const band = screen.getByRole('button', { name: /^2 Mid,/ });
  expect(band).toBeDisabled();
  if (!band.parentElement) throw new Error('The EQ graph is missing its pointer surface');
  fireEvent.pointerDown(band, { pointerId: 72, button: 0 });
  fireEvent.pointerMove(band.parentElement, {
    pointerId: 72,
    clientX: 140,
    clientY: 28,
  });
  expect(apply).toHaveBeenCalledTimes(count);
  const enable = await find('button', { name: 'Enable Equalizer' });
  act(() => enable.focus());
  fireEvent.click(enable);
  await eventually(() =>
    expect(
      screen.getByRole('button', { name: (name) => name.endsWith('Bypass: Processing') }),
    ).toBeEnabled(),
  );
  expect(control).toHaveBeenLastCalledWith(
    'graph-1',
    original.nodes[0]?.id,
    expect.objectContaining({ enabled: true }),
  );
  expect(screen.queryByRole('button', { name: 'Enable Equalizer' })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Disable Equalizer' })).not.toBeInTheDocument();
  expect(
    screen.getByRole('button', { name: (name) => name.endsWith('Bypass: Processing') }),
  ).toHaveFocus();
  for (const input of within(screen.getByRole('region', { name: 'Equalizer' })).getAllByRole(
    'textbox',
  ))
    expect(input).toBeEnabled();
  expect((await port.getGraph()).nodes[0]?.config).toEqual(original.nodes[0]?.config);
  view.unmount();
});

it('recovers a saved Off and muted EQ in separate steps without changing DSP settings', async () => {
  vi.useFakeTimers();
  const created = fixture();
  await ready();
  await addFromMenu('Equalizer');
  const original = lastDocument(created.apply);
  const mutedGraph: RuntimeGraphDocument = {
    ...original,
    nodes: original.nodes.map((node) => ({
      ...node,
      control: { enabled: false, muted: true },
    })),
  };
  created.view.unmount();
  const { port, view } = fixture(mutedGraph, 'graph-1');
  const control = vi.spyOn(port, 'updateNodeControl');
  await ready();
  expect(screen.queryByRole('button', { name: 'Disable Equalizer' })).not.toBeInTheDocument();
  const enable = await find('button', { name: 'Enable Equalizer' });
  act(() => enable.focus());
  fireEvent.click(enable);
  await eventually(() =>
    expect(screen.queryByRole('button', { name: 'Enable Equalizer' })).not.toBeInTheDocument(),
  );
  expect(control).toHaveBeenLastCalledWith(
    'graph-1',
    original.nodes[0]?.id,
    expect.objectContaining({ enabled: true }),
  );
  expect(screen.getByRole('button', { name: 'Unmute Equalizer' })).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Unmute Equalizer' })).toHaveFocus();
  expect((await port.getGraph()).nodes[0]?.control).toEqual({ enabled: true, muted: true });
  fireEvent.click(await find('button', { name: 'Unmute Equalizer' }));
  await eventually(() =>
    expect(screen.queryByRole('button', { name: 'Unmute Equalizer' })).not.toBeInTheDocument(),
  );
  expect(control).toHaveBeenLastCalledWith(
    'graph-1',
    original.nodes[0]?.id,
    expect.objectContaining({ muted: false }),
  );
  expect(screen.queryByRole('button', { name: 'Mute Equalizer' })).not.toBeInTheDocument();
  expect(
    screen.getByRole('button', { name: (name) => name.endsWith('Bypass: Processing') }),
  ).toHaveFocus();
  const recovered = await port.getGraph();
  expect(recovered.nodes[0]?.control).toEqual({ enabled: true, muted: false });
  expect(recovered.nodes[0]?.config).toEqual(original.nodes[0]?.config);
  expect(
    screen.getByRole('button', { name: (name) => name.endsWith('Bypass: Processing') }),
  ).toBeEnabled();
  view.unmount();
});

it.each([
  [
    'Reverb',
    'Small room',
    { decaySeconds: 0.4, damping: 0.6, mix: 0.15 },
    'Decay',
    '2.3',
    'decaySeconds',
    2.3,
  ],
  ['Chorus', 'Wide', { rateHz: 0.8, depthMs: 5, mix: 0.4 }, 'Depth', '7', 'depthMs', 7],
  ['Delay', 'Short echo', { timeMs: 90, feedback: 0.15, mix: 0.2 }, 'Time', '120', 'timeMs', 120],
] as const)(
  'applies a %s preset atomically and separates preset selection from the following knob edit in Undo',
  async (name, preset, parameters, label, text, id, value) => {
    const { apply, parameter, view } = fixture();
    await ready();
    await addFromMenu(name);
    await waitFor(() => expect(apply).toHaveBeenCalledTimes(1));
    const initial = lastDocument(apply).nodes;
    await choose('Preset', preset);
    await waitFor(() => expect(apply).toHaveBeenCalledTimes(2));
    const selected = lastDocument(apply).nodes;
    expect(selected[0]?.config).toMatchObject({ effect: { parameters }, bypass: false });
    expect(selected[0]?.config).not.toHaveProperty('preset');
    expect(screen.getByRole('button', { name: /Preset$/ })).toHaveTextContent(preset);
    const entry = screen.getByRole('textbox', { name: label });
    fireEvent.focus(entry);
    fireEvent.change(entry, { target: { value: text } });
    fireEvent.blur(entry);
    await waitFor(() =>
      expect(lastDocument(apply).nodes[0]?.config).toMatchObject({
        effect: { parameters: { ...parameters, [id]: value } },
      }),
    );
    expect(screen.getByRole('button', { name: /Preset$/ })).toHaveTextContent('Custom');
    await history();
    await waitFor(() => expect(lastDocument(apply).nodes).toEqual(selected));
    expect(screen.getByRole('button', { name: /Preset$/ })).toHaveTextContent(preset);
    await history();
    await waitFor(() => expect(lastDocument(apply).nodes).toEqual(initial));
    expect(screen.getByRole('button', { name: /Preset$/ })).toHaveTextContent('Custom');
    await history(true);
    await waitFor(() => expect(lastDocument(apply).nodes).toEqual(selected));
    expect(parameter).not.toHaveBeenCalled();
    view.unmount();
  },
);

it('reads externally changed effect settings automatically, then edits those current values without a recovery prompt', async () => {
  vi.useFakeTimers();
  const { port, apply, view } = fixture(empty, 'graph-1');
  await ready();
  await addFromMenu('Reverb');
  const document = await port.getGraph();
  const current = {
    ...document,
    revision: document.revision + 1,
    nodes: document.nodes.map((node) => ({
      ...node,
      config: {
        ...(node.config as object),
        effect: {
          kind: 'reverb',
          parameters: {
            decaySeconds: 3,
            damping: 0.6,
            mix: 0.4,
            algorithm: 0,
            roomSize: 1,
            preDelayMs: 0,
          },
        },
      },
    })),
  };
  await act(async () => {
    await port.applyGraph(current);
  });
  await eventually(
    () => expect(screen.getByRole('textbox', { name: 'Damping' })).toHaveValue('0.6'),
    {
      timeout: 2500,
    },
  );
  expect(
    screen.queryByRole('button', { name: 'Overwrite engine with these edits' }),
  ).not.toBeInTheDocument();
  const damping = screen.getByRole('textbox', { name: 'Damping' });
  fireEvent.focus(damping);
  fireEvent.change(damping, { target: { value: '0.7' } });
  fireEvent.blur(damping);
  await eventually(() =>
    expect(lastDocument(apply)).toMatchObject({
      revision: current.revision + 1,
      nodes: [{ config: { effect: { parameters: { damping: 0.7, mix: 0.4, decaySeconds: 3 } } } }],
    }),
  );
  view.unmount();
});
