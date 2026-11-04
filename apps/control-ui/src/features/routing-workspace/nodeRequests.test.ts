import { expect, it } from 'vitest';
import type {
  AudioDeviceDescriptor,
  CapturableApplication,
} from '../../core/domain/control/inventory';
import type {
  ExternalProcessorDescriptor,
  ProviderPorts,
} from '../../core/domain/control/processors';
import { applicationInventorySelection } from './applicationIdentity';
import { chainKinds, effectKinds } from './builtinEffects';
import {
  type RoutingNodeInventory,
  routingNodeRequest,
  supportedChannelCount,
} from './nodeRequests';
import { processorItemId } from './processorCatalogMenu';
import {
  audioInputItemId,
  audioOutputItemId,
  SYSTEM_AUDIO_EXCLUDED_BUNDLE_IDS,
  SYSTEM_AUDIO_ITEM_ID,
} from './routingNodeMenus';

const stereo: ProviderPorts = {
  layoutId: 'layout-v1:stereo',
  inputs: [{ id: 'in', name: 'In', kind: 'main', channels: 2, channelFormat: 'stereo' }],
  outputs: [{ id: 'out', name: 'Out', kind: 'main', channels: 2, channelFormat: 'stereo' }],
};
const processor: ExternalProcessorDescriptor = {
  displayName: 'Fixture Compressor',
  format: 'vst3',
  hasEditor: false,
  layouts: [stereo],
  defaultLayoutId: stereo.layoutId,
  parameters: [],
  typeId: 'vst3:fixture',
  vendor: 'MixRelay Test',
  version: '1.0',
};
const application: CapturableApplication = {
  bundleId: 'com.example.player',
  displayName: 'Player',
  iconDataUrl: null,
  processId: 42,
  selector: { match: 'bundle_id', bundle_id: 'com.example.player', include_descendants: true },
};
const device: AudioDeviceDescriptor = {
  displayName: 'Interface',
  inputChannels: 80,
  isDefaultInput: false,
  isDefaultOutput: false,
  outputChannels: 0.5,
  uid: 'interface',
};
const inventory: RoutingNodeInventory = {
  applications: [application],
  audioDevices: [device],
  processors: [processor],
};

it.each(effectKinds)('creates the built-in %s effect from its menu item', (effect) => {
  expect(routingNodeRequest(`builtin:${effect}`, inventory)).toEqual({ kind: 'effect', effect });
});

it.each(chainKinds)('creates the %s effect chain from its menu item', (preset) => {
  expect(routingNodeRequest(`effect-chain:${preset}`, inventory)).toEqual({
    kind: 'effect_chain',
    preset,
  });
});

it.each(['compressor', 'limiter', 'recorder'] as const)('creates the fixed %s node', (kind) => {
  expect(routingNodeRequest(kind, inventory)).toEqual({ kind });
});

it('creates devices with a supported channel count and applications with their exact selector', () => {
  expect(routingNodeRequest(audioInputItemId(device), inventory)).toEqual({
    kind: 'audio_input',
    channels: 64,
    title: 'Interface',
    uid: 'interface',
  });
  expect(routingNodeRequest(audioOutputItemId(device), inventory)).toEqual({
    kind: 'audio_output',
    channels: 1,
    title: 'Interface',
    uid: 'interface',
  });
  expect(
    routingNodeRequest(applicationInventorySelection(application)?.id ?? '', inventory),
  ).toEqual({
    kind: 'application',
    selector: application.selector,
    title: 'Player',
  });
  expect(routingNodeRequest(processorItemId(processor), inventory)).toEqual({
    kind: 'provider',
    processor,
    ports: stereo,
  });
});

it('offers system audio only where the backend can capture it', () => {
  expect(routingNodeRequest(SYSTEM_AUDIO_ITEM_ID, inventory)).toBeNull();
  expect(
    routingNodeRequest(SYSTEM_AUDIO_ITEM_ID, { ...inventory, systemAudioTitle: 'System audio' }),
  ).toEqual({
    kind: 'application',
    title: 'System audio',
    selector: { match: 'all_except', bundle_ids: [...SYSTEM_AUDIO_EXCLUDED_BUNDLE_IDS] },
  });
});

it.each([
  'builtin:removed-effect',
  'effect-chain:removed-preset',
  'builtin:',
  'unknown',
  'application:missing',
])('resolves a stale or unknown menu item %s to no node', (itemId) => {
  expect(routingNodeRequest(itemId, inventory)).toBeNull();
});

it('clamps reported channel counts to the 1-64 a node supports', () => {
  expect([0, 1, 2.9, 64, 65].map(supportedChannelCount)).toEqual([1, 1, 2, 64, 64]);
});
