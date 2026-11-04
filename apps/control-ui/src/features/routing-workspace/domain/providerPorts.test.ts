import { expect, it } from 'vitest';
import type { ProviderPorts } from '../../../core/domain/control/processors';
import { providerPortsFrom } from './providerPorts';

const ports: ProviderPorts = {
  layoutId: 'stereo',
  inputs: [{ id: 'input', name: 'Input', kind: 'main', channels: 2, channelFormat: 'stereo' }],
  outputs: [{ id: 'output', name: 'Output', kind: 'main', channels: 2, channelFormat: 'stereo' }],
};

it('rejects provider bus geometry outside the native 64-channel direction bound', () => {
  expect(
    providerPortsFrom({
      layoutId: 'layout-v1:too-wide',
      inputs: [
        { id: 'audio', name: 'Main', kind: 'main', channels: 64, channelFormat: 'discrete' },
        { id: 'bus:1', name: 'Aux', kind: 'auxiliary', channels: 1, channelFormat: 'mono' },
      ],
      outputs: [{ id: 'audio', name: 'Main', kind: 'main', channels: 2, channelFormat: 'stereo' }],
    }),
  ).toBeNull();
  expect(
    providerPortsFrom({
      layoutId: 'layout-v1:bad-stereo',
      inputs: [{ id: 'audio', name: 'Main', kind: 'main', channels: 1, channelFormat: 'stereo' }],
      outputs: [{ id: 'output', name: 'Main', kind: 'main', channels: 2, channelFormat: 'stereo' }],
    }),
  ).toBeNull();
  expect(
    providerPortsFrom({
      layoutId: 'layout-v1:empty-name',
      inputs: [
        { id: 'custom-input', name: '', kind: 'main', channels: 2, channelFormat: 'stereo' },
      ],
      outputs: [
        { id: 'custom-output', name: '', kind: 'main', channels: 2, channelFormat: 'stereo' },
      ],
    }),
  ).toEqual({
    layoutId: 'layout-v1:empty-name',
    inputs: [{ id: 'custom-input', name: '', kind: 'main', channels: 2, channelFormat: 'stereo' }],
    outputs: [
      { id: 'custom-output', name: '', kind: 'main', channels: 2, channelFormat: 'stereo' },
    ],
  });
});

it.each([
  ['null layout', null],
  ['empty inputs', { ...ports, inputs: [] }],
  ['empty outputs', { ...ports, outputs: [] }],
  ['extra top-level field', { ...ports, unexpected: true }],
  ['null input bus', { ...ports, inputs: [null] }],
  ['mismatched stereo geometry', { ...ports, inputs: [{ ...ports.inputs[0], channels: 1 }] }],
  ['fractional channel count', { ...ports, inputs: [{ ...ports.inputs[0], channels: 1.5 }] }],
  ['negative channel count', { ...ports, inputs: [{ ...ports.inputs[0], channels: -2 }] }],
  ['unknown bus kind', { ...ports, inputs: [{ ...ports.inputs[0], kind: 'other' }] }],
  [
    'unknown channel format',
    { ...ports, inputs: [{ ...ports.inputs[0], channelFormat: 'other' }] },
  ],
  ['nul bus name', { ...ports, inputs: [{ ...ports.inputs[0], name: 'Input\0' }] }],
  ['extra bus field', { ...ports, inputs: [{ ...ports.inputs[0], extra: 1 }] }],
  ['duplicate bus IDs', { ...ports, inputs: [...ports.inputs, ...ports.inputs] }],
  ['duplicate output IDs', { ...ports, outputs: [...ports.outputs, ...ports.outputs] }],
  ['empty layout identity', { ...ports, layoutId: '' }],
  ['control character identity', { ...ports, layoutId: 'layout\n' }],
  ['oversized layout identity', { ...ports, layoutId: 'é'.repeat(129) }],
  ['non-string layout identity', { ...ports, layoutId: 5 }],
  ['non-array outputs', { ...ports, outputs: null }],
  ['non-string bus identity', { ...ports, inputs: [{ ...ports.inputs[0], id: 4 }] }],
  ['non-string bus name', { ...ports, inputs: [{ ...ports.inputs[0], name: 4 }] }],
  ['oversized bus name', { ...ports, inputs: [{ ...ports.inputs[0], name: 'é'.repeat(513) }] }],
  ['non-number channel count', { ...ports, inputs: [{ ...ports.inputs[0], channels: '2' }] }],
  ['zero channel count', { ...ports, inputs: [{ ...ports.inputs[0], channels: 0 }] }],
  ['more than 64 channels', { ...ports, inputs: [{ ...ports.inputs[0], channels: 65 }] }],
  [
    'too many output buses',
    {
      ...ports,
      outputs: Array.from({ length: 65 }, (_, index) => ({
        ...ports.outputs[0],
        id: String(index),
      })),
    },
  ],
])('rejects %s before materializing effect ports', (_reason, value) => {
  expect(providerPortsFrom(value)).toBeNull();
});

it('rejects a valid-bus list whose combined channel count exceeds the direction bound', () => {
  const oversized = {
    ...ports,
    inputs: [
      { ...ports.inputs[0], id: 'a', channels: 40, channelFormat: 'discrete' },
      { ...ports.inputs[0], id: 'b', channels: 40, channelFormat: 'discrete' },
    ],
  };
  expect(providerPortsFrom(oversized)).toBeNull();
});
