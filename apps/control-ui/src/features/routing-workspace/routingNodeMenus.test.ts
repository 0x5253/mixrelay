import { expect, it } from 'vitest';
import type { AudioGraphNodeMenuItem } from '../../core/audio-graph/audioGraphCanvasTypes';
import type { AudioGraphModel, AudioGraphNode } from '../../core/audio-graph/model';
import type { CapturableApplication } from '../../core/domain/control/inventory';
import type { AudioSessionStatus } from '../../core/domain/control/status';
import { i18n } from '../../core/i18n';
import { routingNodeRequest } from './nodeRequests';
import { createNodeMenuItems } from './routingNodeMenus';
import { unknownNativeFormats, virtualPolicy, virtualSpec } from './virtualEndpointFixtures';

function node(id: string, type: string, config: unknown = {}): AudioGraphNode {
  return {
    id,
    runtime: { type, config },
    position: { x: 0, y: 0 },
    inputPorts: [],
    outputPorts: [],
    appearance: { category: type, icon: 'audio', title: id },
  };
}
function options() {
  const driver = {
    ...node('driver', 'virtual_endpoint', { id: virtualSpec.id }),
    inputPorts: [{ id: 'sink', label: 'sink' }],
  };
  const model: AudioGraphModel = {
    revision: 1,
    edges: [],
    nodes: [
      driver,
      node('app-a', 'application_capture'),
      node('mic', 'audio_device_input'),
      node('effect-a', 'provider'),
      node('effect-b', 'provider'),
      node('recorder', 'recorder'),
      node('monitor', 'audio_device_output', {
        selector: { match: 'persistent_uid', uid: 'monitor' },
      }),
    ],
  };
  const session: AudioSessionStatus = {
    format: { sampleRate: 48000, blockFrames: 128 },
    clockAuthority: 'device',
    observedDeviceSampleRate: 48000,
    issue: null,
    transportLatencyFrames: null,
    capabilities: {
      backend: 'core_audio',
      supportedSampleRates: [48000],
      supportedBlockFrames: [128],
      canReconfigureLive: true,
      hardwareInput: true,
      hardwareOutput: true,
      virtualEndpoint: true,
      applicationCapture: true,
      sampleRateConversion: false,
      driftCorrection: false,
    },
    virtualEndpointSettings: {
      ...virtualPolicy,
      devices: [
        ...virtualPolicy.devices,
        {
          nativeDeviceIds: [],
          nativeFormats: unknownNativeFormats,
          observedSampleRate: 48000,
          supportedSampleRates: [48000],
          canSetSampleRate: true,
          spec: { ...virtualSpec, id: 'other' },
        },
      ],
    },
  };
  return {
    model,
    session,
    t: i18n.getFixedT('en'),
    applications: [],
    processors: [],
    processorEmptyLabel: 'Empty',
    audioDevices: ['monitor', 'third'].map((uid) => ({
      uid,
      displayName: uid,
      inputChannels: 2,
      outputChannels: 2,
      isDefaultInput: false,
      isDefaultOutput: uid === 'monitor',
    })),
  };
}
function item(items: readonly AudioGraphNodeMenuItem[], id: string): AudioGraphNodeMenuItem {
  for (const entry of items) {
    if (entry.id === id) return entry;
    const child = entry.children?.find((value) => value.id === id);
    if (child) return child;
    if (entry.children) {
      const nested = entry.children
        .flatMap((value) => value.children ?? [])
        .find((value) => value.id === id);
      if (nested) return nested;
    }
  }
  throw new Error(`Missing menu item: ${id}`);
}
it('offers additional inputs, outputs, virtual devices, processors and recorders', () => {
  const menus = createNodeMenuItems(options());
  for (const id of [
    'audio-input:monitor',
    'audio-output:third',
    'recorder',
    'virtual-new-input',
    'virtual/input/other',
    'compressor',
    'limiter',
    'add-processor-directory',
    'rescan-processors',
  ]) {
    expect(item(menus, id).disabled).not.toBe(true);
  }
});
it('whole-system capture follows native platform and capture capability', () => {
  const value = options();
  expect(item(createNodeMenuItems(value), 'system-audio').disabled).toBe(false);
  value.session.capabilities.applicationCapture = false;
  expect(item(createNodeMenuItems(value), 'system-audio').disabled).toBe(true);
  value.session.capabilities.backend = 'pipe_wire';
  expect(() => item(createNodeMenuItems(value), 'system-audio')).toThrow(/Missing menu item/);
});

it('deduplicates exact app selectors while upgrading only a missing icon and reports unavailable capture', () => {
  const app: CapturableApplication = {
    displayName: 'Player',
    iconDataUrl: null,
    bundleId: 'org.example.player',
    processId: 10,
    selector: { match: 'bundle_id', bundle_id: 'org.example.player', include_descendants: true },
  };
  const value = options();
  const menus = createNodeMenuItems({
    ...value,
    applications: [
      app,
      { ...app, iconDataUrl: 'data:image/png;base64,first' },
      { ...app, iconDataUrl: 'data:image/png;base64,second' },
    ],
    session: {
      ...value.session,
      capabilities: { ...value.session.capabilities, applicationCapture: false },
    },
  });
  const choice = item(menus, 'application:["bundle","org.example.player"]');
  expect(choice.iconUrl).toBe('data:image/png;base64,first');
  expect(choice.disabled).toBe(true);
  expect(choice.description).toBe(
    'Application capture is unavailable in the current audio session.',
  );
});
it('explains unsupported hardware while allowing another output', () => {
  const value = options();
  const model = {
    ...value.model,
    nodes: [
      node('default-output', 'audio_device_output', { selector: { match: 'default_output' } }),
      node('monitor', 'audio_device_output', {
        selector: { match: 'persistent_uid', uid: 'monitor' },
      }),
    ],
  };
  const menus = createNodeMenuItems({
    ...value,
    model,
    session: {
      ...value.session,
      capabilities: { ...value.session.capabilities, hardwareInput: false },
    },
  });
  expect(item(menus, 'audio-input:monitor').disabled).toBe(true);
  expect(item(menus, 'audio-input:monitor').description).toBe(
    i18n.getFixedT('en')('audioSession.unsupported'),
  );
  expect(item(menus, 'audio-output:third').disabled).toBe(false);
});

it('offers honest empty hardware placeholders and ignores incomplete discovered app selectors without a graph', () => {
  const value = options();
  const menus = createNodeMenuItems({
    ...value,
    model: null,
    audioDevices: [],
    session: undefined,
    applications: [
      {
        displayName: 'Incomplete',
        bundleId: 'broken',
        processId: 1,
        iconDataUrl: null,
        selector: {
          match: 'bundle_id',
          bundle_id: 'mismatched.discovery',
          include_descendants: false,
        },
      },
      {
        displayName: 'Valid',
        bundleId: 'org.example.valid',
        processId: 2,
        iconDataUrl: null,
        selector: {
          match: 'bundle_id',
          bundle_id: 'org.example.valid',
          include_descendants: false,
        },
      },
    ],
  });
  expect(item(menus, 'no-input-devices').disabled).toBe(true);
  expect(item(menus, 'no-output-devices').disabled).toBe(true);
  expect(item(menus, 'application:["bundle","org.example.valid"]').disabled).toBe(false);
  expect(item(menus, 'virtual-new-input').disabled).toBe(true);
});

it('every built-in effect and chain the menu offers resolves to the node it names', () => {
  const value = options();
  const flatten = (items: readonly AudioGraphNodeMenuItem[]): AudioGraphNodeMenuItem[] =>
    items.flatMap((entry) => [entry, ...flatten(entry.children ?? [])]);
  const ids = flatten(createNodeMenuItems(value))
    .map((entry) => entry.id)
    .filter((id) => id.startsWith('builtin:') || id.startsWith('effect-chain:'));
  expect(ids).toHaveLength(17);
  for (const id of ids)
    expect(routingNodeRequest(id, { ...value, processors: [] }), id).toMatchObject({
      kind: id.startsWith('builtin:') ? 'effect' : 'effect_chain',
    });
});
