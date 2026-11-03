import { act, fireEvent, render, renderHook, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { AudioGraphModel } from '../../core/audio-graph/model';
import type { RuntimeGraphDocument } from '../../core/domain/control/graph';
import { useUndoHistory } from '../../core/history/useUndoHistory';
import { MixRelayProvider } from '../../core/theme/MixRelayProvider';
import { AudioNodeCard } from '../../core/ui/AudioNodeCard';
import { graphHistoryGroup } from './graphHistoryGroup';
import { audioGraphToRuntimeGraph, runtimeGraphToAudioGraph, updateRoutingNodeGain } from './model';

const document: RuntimeGraphDocument = {
  schemaVersion: 2,
  revision: 1,
  edges: [],
  nodes: [
    {
      id: 'virtual',
      label: null,
      type: 'virtual_endpoint',
      config: {
        reference: { render: true, capture: true },
        id: 'virtual',
        displayName: 'MixRelay Virtual I/O',
        defaultSampleRate: 48000,
        supportedSampleRates: [48000],
        render: { channelLayout: 'discrete', channels: 64 },
        capture: { channelLayout: 'discrete', channels: 16 },
      },
    },
  ],
};
const sourceId = 'virtual::host-source';
const sinkId = 'virtual::host-sink';

afterEach(() => vi.useRealTimers());

it('provides virtual source gain alongside endpoint options, without an output gain control', () => {
  const model = runtimeGraphToAudioGraph(document);
  const source = model.nodes.find((node) => node.id === sourceId);
  const sink = model.nodes.find((node) => node.id === sinkId);
  expect(source?.appearance.gainControl?.valueDb).toBe(0);
  expect(sink?.appearance.gainControl).toBeUndefined();
  if (!source) throw new Error('source missing');
  const onGain = vi.fn();
  const onEndpoint = vi.fn();
  render(
    <MixRelayProvider>
      <AudioNodeCard
        {...source.appearance}
        inputCount={0}
        outputCount={64}
        onSelect={vi.fn()}
        onGainChange={onGain}
        onEndpointChange={onEndpoint}
      />
    </MixRelayProvider>,
  );
  fireEvent.click(screen.getByRole('button', { name: (name) => name.endsWith('Options') }));
  expect(screen.getByRole('textbox', { name: 'Device name' })).toBeInTheDocument();
  expect(screen.getAllByRole('button', { name: (name) => name.endsWith('Options') })).toHaveLength(
    1,
  );
  expect(screen.getByRole('button', { name: /Channels$/ })).toHaveTextContent('Mixed');
  expect(onEndpoint).not.toHaveBeenCalled();
  const slider = screen.getByRole('slider', { name: 'Source gain' });
  fireEvent.keyDown(slider, { key: 'ArrowLeft' });
  expect(onGain).toHaveBeenCalledWith(-0.5);
});

it.each([
  {
    kind: 'application capture',
    type: 'application_capture',
    config: {
      gainDb: -6,
      selector: { bundle_id: 'com.example.player', include_descendants: true, match: 'bundle_id' },
      sourcePlayback: 'preserve',
      stream: { channelLayout: 'stereo', channels: 2 },
    },
  },
  {
    kind: 'device input',
    type: 'audio_device_input',
    config: {
      gainDb: -6,
      outputChannels: 1,
      selector: { match: 'persistent_uid', uid: 'fixture-device' },
    },
  },
] as const)('binds $kind gain to its source config and card control', ({ type, config }) => {
  const sourceDocument: RuntimeGraphDocument = {
    schemaVersion: 2,
    revision: 3,
    edges: [],
    nodes: [{ id: 'source', label: 'Source', type, config }],
  };
  const model = runtimeGraphToAudioGraph(sourceDocument);
  const source = model.nodes[0];
  expect(source?.appearance.gainControl).toEqual({ label: 'Source gain', valueDb: -6 });
  if (!source) throw new Error('source missing');
  const onGain = vi.fn();
  render(
    <MixRelayProvider>
      <AudioNodeCard
        {...source.appearance}
        inputCount={source.inputPorts.length}
        outputCount={source.outputPorts.length}
        onSelect={vi.fn()}
        onGainChange={onGain}
      />
    </MixRelayProvider>,
  );
  fireEvent.click(screen.getByRole('button', { name: (name) => name.endsWith('Options') }));
  fireEvent.keyDown(screen.getByRole('slider', { name: 'Source gain' }), { key: 'ArrowRight' });
  expect(onGain).toHaveBeenCalledWith(-5.5);
  const changed = updateRoutingNodeGain(model, source.id, -5.5);
  const saved = audioGraphToRuntimeGraph(changed, sourceDocument);
  expect(saved.nodes[0]).toMatchObject({ type, config: { ...config, gainDb: -5.5 } });
});

it('serializes the source gain once even when the sink is visited last or first', () => {
  const before = runtimeGraphToAudioGraph(document);
  const changed = updateRoutingNodeGain(before, sourceId, -12);
  expect(changed.nodes.find((node) => node.id === sinkId)).toBe(
    before.nodes.find((node) => node.id === sinkId),
  );
  for (const nodes of [changed.nodes, [...changed.nodes].reverse()]) {
    const saved = audioGraphToRuntimeGraph({ ...changed, nodes }, document);
    expect(saved.nodes).toHaveLength(1);
    expect(saved.nodes[0]?.config).toMatchObject({
      gainDb: -12,
      render: { channels: 64 },
      capture: { channels: 16 },
    });
    const restored = runtimeGraphToAudioGraph(JSON.parse(JSON.stringify(saved)));
    expect(
      restored.nodes.find((node) => node.id === sourceId)?.appearance.gainControl?.valueDb,
    ).toBe(-12);
  }
});

it('coalesces virtual gain gestures and roundtrips undo/redo through the runtime document', () => {
  vi.useFakeTimers();
  const { result } = renderHook(() => useUndoHistory<AudioGraphModel, 'level'>());
  let current = runtimeGraphToAudioGraph(document);
  act(() => {
    for (const gain of [-1, -2, -6, -12]) {
      const next = updateRoutingNodeGain(current, sourceId, gain);
      const group = graphHistoryGroup(current, next, 'level');
      if (!group) throw new Error('virtual gain must have a control history group');
      result.current.recordDebounced(current, next, 'level', group);
      current = next;
      vi.advanceTimersByTime(20);
    }
  });
  const undone = result.current.undo();
  expect(
    undone?.value.nodes.find((node) => node.id === sourceId)?.appearance.gainControl?.valueDb,
  ).toBe(0);
  expect(result.current.undo()).toBeNull();
  const redone = result.current.redo();
  if (!redone) throw new Error('redo missing');
  expect(audioGraphToRuntimeGraph(redone.value, document).nodes[0]?.config).toMatchObject({
    gainDb: -12,
  });
  expect(vi.getTimerCount()).toBe(0);
});
