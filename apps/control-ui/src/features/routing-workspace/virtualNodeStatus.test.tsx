import { StorybookMockControlPort } from '../../core/adapters/fake/StorybookMockControlPort';
import type { RuntimeGraphDocument } from '../../core/domain/control/graph';
import type { AudioSessionStatus } from '../../core/domain/control/status';
import '../../test-support/integration';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { I18nextProvider } from 'react-i18next';
import { describe, expect, it, vi } from 'vitest';
import type { AudioGraphModel } from '../../core/audio-graph/model';
import { i18n } from '../../core/i18n';
import { MixRelayProvider } from '../../core/theme/MixRelayProvider';
import { AudioNodeCard } from '../../core/ui/AudioNodeCard';
import { audioGraphToRuntimeGraph, runtimeGraphToAudioGraph } from './model';
import { RoutingWorkspace } from './RoutingWorkspace';
import { type GraphAvailability, presentVirtualNodeStatus } from './virtualNodeStatus';

vi.mock('../../core/audio-graph/AudioGraphCanvas', () => ({
  AudioGraphCanvas: ({ model }: { model: AudioGraphModel }) => (
    <output data-testid="presented-status">
      {JSON.stringify(
        model.nodes.map((node) => [node.appearance.status, node.appearance.statusTone]),
      )}
    </output>
  ),
}));

const document: RuntimeGraphDocument = {
  schemaVersion: 2,
  revision: 1,
  edges: [],
  nodes: [
    {
      id: 'virtual',
      type: 'virtual_endpoint',
      label: null,
      config: {
        reference: { render: true, capture: true },
        id: 'virtual',
        displayName: 'MixRelay',
        defaultSampleRate: 48000,
        supportedSampleRates: [48000],
        render: { channels: 2, channelLayout: 'stereo' },
        capture: { channels: 2, channelLayout: 'stereo' },
      },
    },
  ],
};
const running: AudioSessionStatus = {
  processingState: 'running',
  issue: null,
  format: { sampleRate: 48000, blockFrames: 128 },
  clockAuthority: 'engine_timer',
  observedDeviceSampleRate: null,
  transportLatencyFrames: null,
  capabilities: {
    backend: 'core_audio',
    supportedSampleRates: [48000],
    supportedBlockFrames: [128],
    canReconfigureLive: false,
    hardwareInput: true,
    hardwareOutput: true,
    virtualEndpoint: true,
    applicationCapture: true,
    sampleRateConversion: false,
    driftCorrection: false,
  },
};

const cases: Array<[string, GraphAvailability, AudioSessionStatus | undefined, string, string]> = [
  ['healthy without signal', 'active', running, 'positive', 'routing.nodeStatus.running'],
  [
    'stopped',
    'active',
    { ...running, processingState: 'stopped' },
    'critical',
    'routing.nodeStatus.stopped',
  ],
  [
    'failed',
    'active',
    { ...running, issue: 'processing_failed' },
    'critical',
    'routing.nodeStatus.failed',
  ],
  ['pending apply', 'applying', running, 'warning', 'routing.states.applying'],
  [
    'failed while applying',
    'applying',
    { ...running, issue: 'processing_failed' },
    'critical',
    'routing.nodeStatus.failed',
  ],
  [
    'stopped while loading',
    'loading',
    { ...running, processingState: 'stopped' },
    'critical',
    'routing.nodeStatus.stopped',
  ],
  ['loading', 'loading', running, 'warning', 'routing.states.loading'],
  ['unavailable', 'unavailable', running, 'critical', 'routing.states.unavailable'],
  ['blocked draft', 'blocked', running, 'warning', 'routing.states.blocked'],
  ['missing session', 'active', undefined, 'warning', 'routing.nodeStatus.unknown'],
  [
    'old engine',
    'active',
    { ...running, processingState: undefined },
    'warning',
    'routing.nodeStatus.unknown',
  ],
  [
    'idle',
    'active',
    { ...running, processingState: 'idle' },
    'positive',
    'routing.nodeStatus.idle',
  ],
  [
    'missing app',
    'active',
    { ...running, issue: 'application_unavailable' },
    'warning',
    'routing.nodeStatus.issues.application_unavailable',
  ],
  [
    'ambiguous app',
    'active',
    { ...running, issue: 'application_ambiguous' },
    'warning',
    'routing.nodeStatus.issues.application_ambiguous',
  ],
  [
    'clock warning',
    'active',
    { ...running, issue: 'clock_not_synchronized' },
    'warning',
    'routing.nodeStatus.issues.clock_not_synchronized',
  ],
  [
    'settings warning',
    'active',
    { ...running, endpointPreferencesUnavailable: true },
    'warning',
    'routing.nodeStatus.issues.preferences_unavailable',
  ],
];

describe.each(['en', 'ko-KR', 'pseudo'])('virtual node session status: %s', (locale) => {
  it.each(cases)(
    '%s is qualified from graph/session, not meters',
    (_name, state, session, tone, key) => {
      const raw = runtimeGraphToAudioGraph(document);
      const before = JSON.stringify(raw);
      expect(raw.nodes.every((node) => node.appearance.statusTone !== 'positive')).toBe(true);
      const t = i18n.getFixedT(locale);
      const result = presentVirtualNodeStatus(raw, state, session, t);
      expect(result.nodes).toHaveLength(2);
      for (const node of result.nodes) {
        expect(node.appearance.statusTone).toBe(tone);
        expect(node.appearance.status).toBe(t(key));
        expect(node.appearance.status).not.toBe(key);
      }
      expect(JSON.stringify(raw)).toBe(before);
      expect(audioGraphToRuntimeGraph(result, document)).toEqual(
        audioGraphToRuntimeGraph(raw, document),
      );
    },
  );

  it('renders a minimal localized tooltip, with no visible Ready text', async () => {
    const t = i18n.getFixedT(locale);
    const raw = runtimeGraphToAudioGraph(document);
    const appearance = presentVirtualNodeStatus(raw, 'active', running, t).nodes[0]?.appearance;
    if (!appearance) throw new Error('missing virtual input');
    const view = render(
      <MixRelayProvider>
        <AudioNodeCard {...appearance} inputCount={0} outputCount={2} onSelect={vi.fn()} />
      </MixRelayProvider>,
    );
    const marker = screen.getByRole('img', { name: t('routing.nodeStatus.running') });
    expect(marker).toHaveAttribute('data-tone', 'positive');
    expect(screen.queryByText(t('routing.nodeStatus.running'))).not.toBeInTheDocument();
    fireEvent.pointerMove(window.document.body);
    fireEvent.pointerOver(marker);
    expect(await screen.findByRole('tooltip')).toHaveTextContent(t('routing.nodeStatus.running'));
    view.unmount();
  });
});

it('updates the workspace after stop/failure/recovery without publishing a graph or inferring silence', async () => {
  await i18n.changeLanguage('en');
  const port = new StorybookMockControlPort({
    graph: structuredClone(document),
    graphStatus: { protocolVersion: 1, activeRevision: 1, submittedRevision: 1 },
    meters: { protocolVersion: 1, revision: 1, nodes: [] },
    status: {
      protocolVersion: 1,
      driverState: 'ready',
      sampleRate: 48000,
      bufferFrames: 128,
      underruns: 0,
      overruns: 0,
      audioSession: running,
    },
  });
  const apply = vi.spyOn(port, 'applyGraph');
  const workspace = (session?: AudioSessionStatus) => (
    <MixRelayProvider>
      <I18nextProvider i18n={i18n}>
        <RoutingWorkspace controlPort={port} audioSession={session} />
      </I18nextProvider>
    </MixRelayProvider>
  );
  const view = render(workspace());
  const status = await screen.findByTestId('presented-status');
  await waitFor(() => expect(status).toHaveTextContent('Audio status unknown'));
  view.rerender(workspace(running));
  expect(status).toHaveTextContent('positive');
  view.rerender(workspace({ ...running, processingState: 'stopped' }));
  expect(status).toHaveTextContent('Audio processing stopped');
  expect(status).toHaveTextContent('critical');
  view.rerender(workspace({ ...running, issue: 'processing_failed' }));
  expect(status).toHaveTextContent('Audio processing failed');
  view.rerender(workspace(running));
  expect(status).toHaveTextContent('Audio session running');
  expect(status).not.toHaveTextContent('critical');
  expect(apply).not.toHaveBeenCalled();
  view.unmount();
});
