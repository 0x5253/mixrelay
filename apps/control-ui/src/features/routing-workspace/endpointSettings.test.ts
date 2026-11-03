import { expect, it } from 'vitest';
import type { RuntimeGraphDocument } from '../../core/domain/control/graph';
import type { VirtualEndpointSettings } from '../../core/domain/control/status';
import { addRoutingNode, audioGraphToRuntimeGraph, runtimeGraphToAudioGraph } from './model';
import { unknownNativeFormats, virtualPolicy, virtualSpec } from './virtualEndpointFixtures';

const empty: RuntimeGraphDocument = { schemaVersion: 2, revision: 1, nodes: [], edges: [] };
const spec = virtualSpec;
const policy = virtualPolicy;

function observed(
  settings: VirtualEndpointSettings,
  value: number | null,
  canSetSampleRate = true,
): VirtualEndpointSettings {
  return {
    ...settings,
    devices: [
      {
        nativeDeviceIds: [],
        nativeFormats: unknownNativeFormats,
        spec,
        observedSampleRate: value,
        supportedSampleRates: [44100, 48000],
        canSetSampleRate,
      },
    ],
  };
}

it.each(['virtual_input', 'virtual_output'] as const)(
  'creates %s using the device template, not a graph-wide sample rate',
  (kind) => {
    const model = addRoutingNode(
      runtimeGraphToAudioGraph(empty),
      { kind, spec },
      { x: 0, y: 0 },
      undefined,
      observed(policy, null),
    );
    expect(audioGraphToRuntimeGraph(model, empty).nodes[0]?.config).toMatchObject({
      defaultSampleRate: 44100,
      supportedSampleRates: [44100, 48000],
    });
    const explicit = addRoutingNode(
      runtimeGraphToAudioGraph(empty),
      { kind, spec },
      { x: 0, y: 0 },
      undefined,
    );
    expect(audioGraphToRuntimeGraph(explicit, empty).nodes[0]?.config).toMatchObject({
      defaultSampleRate: 44100,
      supportedSampleRates: [44100, 48000],
    });
  },
);

it.each(['virtual_input', 'virtual_output'] as const)(
  'restores authoritative identity, name, rate and asymmetric channels: %s',
  (kind) => {
    const first = addRoutingNode(
      runtimeGraphToAudioGraph(empty),
      { kind, spec },
      { x: 0, y: 0 },
      undefined,
      policy,
    );
    const serialized = audioGraphToRuntimeGraph(first, empty);
    const both = addRoutingNode(
      runtimeGraphToAudioGraph(serialized),
      {
        kind: kind === 'virtual_input' ? 'virtual_output' : 'virtual_input',
        spec,
      },
      { x: 400, y: 0 },
      undefined,
      policy,
    );
    const restored = audioGraphToRuntimeGraph(both, serialized);
    expect(restored.nodes).toHaveLength(1);
    expect(restored.nodes[0]).toMatchObject({
      id: serialized.nodes[0]?.id,
      config: spec,
    });
    expect(both.nodes.find((node) => node.outputPorts.length)?.outputPorts).toHaveLength(16);
    expect(both.nodes.find((node) => node.inputPorts.length)?.inputPorts).toHaveLength(64);
    expect(addRoutingNode(both, { kind, spec }, { x: 0, y: 0 }, undefined, policy)).toBe(both);
  },
);

it('refuses unsupported creation, not just disabled UI clicks', () => {
  const model = runtimeGraphToAudioGraph(empty);
  const fixed = {
    ...policy,
    canCreate: false,
    devices: [],
    canRename: false,
    renderChannels: [64],
    captureChannels: [16],
  };
  expect(
    addRoutingNode(model, { kind: 'virtual_input', spec }, { x: 0, y: 0 }, undefined, fixed),
  ).toBe(model);
  expect(
    addRoutingNode(model, { kind: 'virtual_output', spec }, { x: 0, y: 0 }, undefined, {
      ...policy,
      canCreate: false,
      devices: [],
      creationTemplate: null,
    }),
  ).toBe(model);
});
