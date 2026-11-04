import { StorybookMockControlPort } from '../../core/adapters/fake/StorybookMockControlPort';
import type { RuntimeGraphDocument } from '../../core/domain/control/graph';
import '../../test-support/integration';
import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { AudioGraphCanvasProps } from '../../core/audio-graph/AudioGraphCanvas';
import { I18nProvider, i18n } from '../../core/i18n';
import { MixRelayProvider } from '../../core/theme/MixRelayProvider';
import { AudioNodeCard } from '../../core/ui/AudioNodeCard';
import { advance, eventually, find, findIn } from '../../test-support/fakeTimerQueries';
import { RoutingWorkspace } from './RoutingWorkspace';
import { addRoutingNode } from './routingNodeCreation';
import { audioGraphToRuntimeGraph, runtimeGraphToAudioGraph } from './runtimeGraphProjection';

// Off, Mute and Bypass behaviour is unit-tested (signalControls, nodeControl and
// workspaceEdits). This covers where keyboard focus goes as the workspace applies each
// recovery command through the real Core node header.
vi.mock('../../core/audio-graph/AudioGraphCanvas', () => ({
  AudioGraphCanvas: (props: AudioGraphCanvasProps) => (
    <section aria-label={props.ariaLabel}>
      {props.model.nodes.map((node) => {
        const controls = props.renderNodeControls?.(node);
        return (
          <section key={node.id} aria-label={node.appearance.title}>
            <AudioNodeCard
              {...node.appearance}
              parameterControls={node.appearance.parameterControls?.filter(
                (control) => control.id === 'bypass',
              )}
              optionsLabel={i18n.t('routing.nodes.options')}
              headerActions={controls?.power}
              muteControl={controls?.mute}
              poweredOff={node.runtime?.control?.enabled === false}
              faceMode="compact"
              inputCount={node.inputPorts.length}
              outputCount={node.outputPorts.length}
              onParameterChange={(id, value) => props.onNodeParameterChange?.(node.id, id, value)}
            />
          </section>
        );
      })}
    </section>
  ),
}));

const empty: RuntimeGraphDocument = { schemaVersion: 2, revision: 1, nodes: [], edges: [] };

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it('recovers a stored Off and Mute in separate steps and moves focus on to Bypass', async () => {
  vi.useFakeTimers();
  const created = audioGraphToRuntimeGraph(
    addRoutingNode(runtimeGraphToAudioGraph(empty), { kind: 'compressor' }, { x: 0, y: 0 }),
    empty,
  );
  const graph = {
    ...created,
    nodes: created.nodes.map((node) => ({
      ...node,
      config: { ...(node.config as object), bypass: true },
      control: { enabled: false, muted: true },
    })),
  };
  const port = new StorybookMockControlPort({
    graph,
    graphStatus: { protocolVersion: 1, activeRevision: 1, submittedRevision: 1 },
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
  render(
    <MixRelayProvider>
      <I18nProvider>
        <RoutingWorkspace controlPort={port} graphId="graph-1" />
      </I18nProvider>
    </MixRelayProvider>,
  );
  await find('region', { name: 'MixRelay virtual I/O routing graph' }, { timeout: 5000 });
  await advance();
  const card = within(screen.getByRole('region', { name: 'Compressor' }));
  const bypassed = () =>
    card.getByRole('button', { name: (name) => name.endsWith('Bypass: Bypassed') });
  expect(card.queryByRole('button', { name: 'Mute Compressor' })).not.toBeInTheDocument();
  expect(bypassed()).toBeDisabled();
  const enable = await findIn(card, 'button', { name: 'Enable Compressor' });
  act(() => enable.focus());
  fireEvent.click(enable);
  await eventually(() =>
    expect(card.queryByRole('button', { name: 'Enable Compressor' })).not.toBeInTheDocument(),
  );
  const unmute = card.getByRole('button', { name: 'Unmute Compressor' });
  expect(unmute).toHaveFocus();
  fireEvent.click(unmute);
  await eventually(() =>
    expect(card.queryByRole('button', { name: 'Unmute Compressor' })).not.toBeInTheDocument(),
  );
  expect(bypassed()).toBeEnabled();
  expect(bypassed()).toHaveFocus();
  const restored = (await port.getGraph()).nodes[0];
  expect(restored?.control).toEqual({ enabled: true, muted: false });
  expect(restored?.config).toEqual(graph.nodes[0]?.config);
});
