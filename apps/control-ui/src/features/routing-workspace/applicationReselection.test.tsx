import { StorybookMockControlPort } from '../../core/adapters/fake/StorybookMockControlPort';
import type { RuntimeGraphDocument } from '../../core/domain/control/graph';
import type { CapturableApplication } from '../../core/domain/control/inventory';
import '../../test-support/integration';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { I18nextProvider } from 'react-i18next';
import { expect, it, vi } from 'vitest';
import type { AudioGraphModel } from '../../core/audio-graph/model';
import { i18n } from '../../core/i18n';
import { MixRelayProvider } from '../../core/theme/MixRelayProvider';
import { AudioNodeCard } from '../../core/ui/AudioNodeCard';
import {
  addRoutingNode,
  audioGraphToRuntimeGraph,
  runtimeGraphToAudioGraph,
  updateRoutingNodeGain,
} from './model';
import { RoutingWorkspace } from './RoutingWorkspace';
import { presentApplicationReselection } from './virtualNodeStatus';

vi.mock('../../core/audio-graph/AudioGraphCanvas', () => ({
  AudioGraphCanvas: ({
    model,
    nodeMenuItems,
    onAddNode,
  }: {
    model: AudioGraphModel;
    nodeMenuItems?: unknown[];
    onAddNode?: (itemId: string, position: { x: number; y: number }) => void;
  }) => (
    <>
      <output data-testid="status">
        {model.nodes.map((node) => node.appearance.status).join(', ')}
      </output>
      <output data-testid="application-presentation">
        {JSON.stringify(
          model.nodes.map((node) => ({
            iconUrl: node.appearance.iconUrl ?? null,
            title: node.appearance.title,
          })),
        )}
      </output>
      <output data-testid="menu-items">{JSON.stringify(nodeMenuItems)}</output>
      <button
        type="button"
        onClick={() =>
          onAddNode?.(`application:${JSON.stringify(['bundle', 'org.example.player'])}`, {
            x: 10,
            y: 20,
          })
        }
      >
        select malformed inventory item
      </button>
    </>
  ),
}));

const document: RuntimeGraphDocument = {
  schemaVersion: 2,
  revision: 1,
  edges: [],
  nodes: [
    {
      id: 'legacy',
      label: 'Old application',
      type: 'application_capture',
      config: {
        requiresReselection: true,
        selector: { match: 'process_id', process_id: 123, include_descendants: true },
        gainDb: 0,
        sourcePlayback: 'preserve',
        stream: { channels: 2, channelLayout: 'stereo' },
      },
    },
  ],
};

it.each(['en', 'ko-KR', 'pseudo'])(
  'shows a minimal actionable restored-PID tooltip in %s',
  async (locale) => {
    const raw = runtimeGraphToAudioGraph(document);
    const before = JSON.stringify(raw);
    const t = i18n.getFixedT(locale);
    const node = presentApplicationReselection(raw, t).nodes[0];
    if (!node) throw new Error('missing application');
    const view = render(
      <MixRelayProvider>
        <AudioNodeCard {...node.appearance} inputCount={0} outputCount={2} onSelect={vi.fn()} />
      </MixRelayProvider>,
    );
    const marker = screen.getByRole('img', { name: t('routing.nodeStatus.reselectApplication') });
    expect(marker).toHaveAttribute('data-tone', 'warning');
    fireEvent.pointerMove(window.document.body);
    fireEvent.pointerOver(marker);
    expect(await screen.findByRole('tooltip')).toHaveTextContent(
      t('routing.nodeStatus.reselectApplication'),
    );
    expect(JSON.stringify(raw)).toBe(before);
    view.unmount();
  },
);

it('retains the native safety flag and PID through gain edits and serialization', () => {
  const raw = runtimeGraphToAudioGraph(document);
  const presented = presentApplicationReselection(raw, i18n.getFixedT('en'));
  const edited = updateRoutingNodeGain(presented, 'legacy', -6);
  const saved = audioGraphToRuntimeGraph(edited, document);
  expect(saved.nodes[0]?.config).toMatchObject({
    requiresReselection: true,
    gainDb: -6,
    selector: { match: 'process_id', process_id: 123, include_descendants: true },
  });
  expect(saved.nodes[0]?.config).not.toHaveProperty('status');
});

it('clears the warning only for the fresh application selection, without mutating the old document', () => {
  const replacement = addRoutingNode(
    { revision: 1, edges: [], nodes: [] },
    {
      kind: 'application',
      title: 'Chosen app',
      selector: { match: 'process_id', process_id: 456, include_descendants: true },
    },
    { x: 10, y: 20 },
  );
  const node = presentApplicationReselection(replacement, i18n.getFixedT('en')).nodes[0];
  expect(node?.appearance.status).toBeUndefined();
  expect(node?.runtime?.config).not.toHaveProperty('requiresReselection');
  expect(document.nodes[0]?.config).toHaveProperty('requiresReselection', true);
});

it('presents the restored application in the actual workspace without publishing an audio edit', async () => {
  await i18n.changeLanguage('en');
  const port = new StorybookMockControlPort({
    applications: [
      {
        bundleId: 'org.example.other',
        displayName: 'Different application',
        iconDataUrl: 'data:image/png;base64,different',
        processId: 123,
        selector: { match: 'process_id', process_id: 123, include_descendants: true },
      },
    ],
    graph: structuredClone(document),
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
  const apply = vi.spyOn(port, 'applyGraph');
  const view = render(
    <MixRelayProvider>
      <I18nextProvider i18n={i18n}>
        <RoutingWorkspace controlPort={port} />
      </I18nextProvider>
    </MixRelayProvider>,
  );
  expect(await screen.findByTestId('status')).toHaveTextContent('Select the application again');
  expect(await screen.findByTestId('application-presentation')).toHaveTextContent(
    JSON.stringify([{ iconUrl: null, title: 'Old application' }]),
  );
  expect(apply).not.toHaveBeenCalled();
  expect((await port.getGraph()).nodes[0]?.config).toHaveProperty('requiresReselection', true);
  view.unmount();
});

it('updates only the icon when an exact app disappears and reappears in inventory', async () => {
  await i18n.changeLanguage('en');
  const graph: RuntimeGraphDocument = {
    schemaVersion: 2,
    revision: 3,
    edges: [],
    nodes: [
      {
        id: 'current',
        label: 'My saved name',
        type: 'application_capture',
        config: {
          selector: { match: 'process_id', process_id: 20, include_descendants: true },
          gainDb: -6,
          sourcePlayback: 'preserve',
          stream: { channels: 2, channelLayout: 'stereo' },
        },
      },
    ],
  };
  const port = new StorybookMockControlPort({
    graph,
    graphStatus: { protocolVersion: 1, activeRevision: 3, submittedRevision: 3 },
    status: {
      protocolVersion: 1,
      driverState: 'ready',
      sampleRate: 48000,
      bufferFrames: 128,
      underruns: 0,
      overruns: 0,
    },
  });
  let inventory: CapturableApplication[] = [];
  vi.spyOn(port, 'listCapturableApplications').mockImplementation(async () => inventory);
  const apply = vi.spyOn(port, 'applyGraph');
  const view = render(
    <MixRelayProvider>
      <I18nextProvider i18n={i18n}>
        <RoutingWorkspace controlPort={port} />
      </I18nextProvider>
    </MixRelayProvider>,
  );
  const presentation = await screen.findByTestId('application-presentation');
  expect(presentation).toHaveTextContent(
    JSON.stringify([{ iconUrl: null, title: 'My saved name' }]),
  );
  inventory = [
    {
      bundleId: 'org.example.player',
      displayName: 'Renamed by OS',
      iconDataUrl: 'data:image/png;base64,reappeared',
      processId: 20,
      selector: { match: 'process_id', process_id: 20, include_descendants: true },
    },
  ];
  window.dispatchEvent(new Event('focus'));
  await waitFor(() =>
    expect(presentation).toHaveTextContent(
      JSON.stringify([{ iconUrl: 'data:image/png;base64,reappeared', title: 'My saved name' }]),
    ),
  );
  inventory = [];
  window.dispatchEvent(new Event('focus'));
  await waitFor(() =>
    expect(presentation).toHaveTextContent(
      JSON.stringify([{ iconUrl: null, title: 'My saved name' }]),
    ),
  );
  expect(apply).not.toHaveBeenCalled();
  expect(await port.getGraph()).toEqual(graph);
  view.unmount();
});

it('keeps menu and node identity aligned while refusing a malformed inventory choice', async () => {
  await i18n.changeLanguage('en');
  const graph: RuntimeGraphDocument = {
    schemaVersion: 2,
    revision: 1,
    edges: [],
    nodes: [
      {
        id: 'selected',
        label: 'My saved bundle title',
        type: 'application_capture',
        config: {
          selector: {
            match: 'bundle_id',
            bundle_id: 'org.example.dupe',
            include_descendants: false,
          },
          gainDb: -6,
          sourcePlayback: 'preserve',
          stream: { channels: 2, channelLayout: 'stereo' },
        },
      },
    ],
  };
  const port = new StorybookMockControlPort({
    applications: [
      {
        bundleId: 'org.example.player',
        displayName: 'Malformed application',
        iconDataUrl: null,
        processId: 10,
        selector: { match: 'process_id', process_id: 0, include_descendants: true },
      },
      {
        bundleId: 'org.example.valid',
        displayName: 'Valid application',
        iconDataUrl: null,
        processId: 20,
        selector: { match: 'process_id', process_id: 20, include_descendants: true },
      },
      {
        bundleId: 'org.example.dupe',
        displayName: 'Duplicate first',
        iconDataUrl: 'data:image/png;base64,first',
        processId: 30,
        selector: {
          match: 'bundle_id',
          bundle_id: 'org.example.dupe',
          include_descendants: false,
        },
      },
      {
        bundleId: 'org.example.dupe',
        displayName: 'Duplicate second',
        iconDataUrl: 'data:image/png;base64,second',
        processId: 40,
        selector: {
          match: 'bundle_id',
          bundle_id: 'org.example.dupe',
          include_descendants: false,
        },
      },
    ],
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
  });
  const apply = vi.spyOn(port, 'applyGraph');
  const view = render(
    <MixRelayProvider>
      <I18nextProvider i18n={i18n}>
        <RoutingWorkspace controlPort={port} />
      </I18nextProvider>
    </MixRelayProvider>,
  );
  const items = await screen.findByTestId('menu-items');
  await waitFor(() => expect(items).toHaveTextContent('Valid application'));
  expect(items).not.toHaveTextContent('Malformed application');
  expect(items).toHaveTextContent('Duplicate first');
  expect(items).not.toHaveTextContent('Duplicate second');
  expect(items).toHaveTextContent('data:image/png;base64,first');
  expect(items).not.toHaveTextContent('data:image/png;base64,second');
  expect(await screen.findByTestId('application-presentation')).toHaveTextContent(
    JSON.stringify([{ iconUrl: 'data:image/png;base64,first', title: 'My saved bundle title' }]),
  );
  fireEvent.click(screen.getByRole('button', { name: 'select malformed inventory item' }));
  expect(apply).not.toHaveBeenCalled();
  expect(await port.getGraph()).toEqual(graph);
  view.unmount();
});
