import { act, renderHook } from '@testing-library/react';
import { type ComponentProps, isValidElement, useLayoutEffect, useRef } from 'react';
import { expect, it, vi } from 'vitest';
import { StorybookMockControlPort } from '../../core/adapters/fake/StorybookMockControlPort';
import type { RuntimeGraphDocument } from '../../core/domain/control/graph';
import type { VirtualEndpointSpec } from '../../core/domain/control/status';
import type { NameDialog } from '../../core/ui/NameDialog';
import { runtimeGraphToAudioGraph } from './model';
import { useVirtualEndpointCreation } from './useVirtualEndpointCreation';
import { virtualPolicy, virtualSpec } from './virtualEndpointFixtures';
import { virtualEndpointItemId } from './virtualEndpointMenu';

const graph: RuntimeGraphDocument = { schemaVersion: 2, revision: 1, nodes: [], edges: [] };
function args() {
  return {
    owner: new StorybookMockControlPort({
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
    }),
    model: runtimeGraphToAudioGraph(graph),
    settings: virtualPolicy,
    enabled: true,
    updateModel: vi.fn(),
    createDevice: vi
      .fn<(spec: VirtualEndpointSpec) => Promise<VirtualEndpointSpec | null>>()
      .mockImplementation(async (spec) => spec),
  };
}
function dialog(value: ReturnType<typeof useVirtualEndpointCreation>['dialog']) {
  if (!isValidElement<ComponentProps<typeof NameDialog>>(value))
    throw new Error('Expected the named-creation dialog');
  return value.props;
}
it('declines unrelated menus and safely consumes unsupported virtual identities', () => {
  const value = args();
  const { result, rerender } = renderHook(({ options }) => useVirtualEndpointCreation(options), {
    initialProps: { options: value },
  });
  act(() => {
    expect(result.current.tryAdd('compressor', { x: 0, y: 0 })).toBe(false);
    expect(result.current.tryAdd('virtual-unknown', { x: 0, y: 0 })).toBe(true);
  });
  expect(result.current.dialog).toBeNull();
  rerender({ options: { ...value, settings: { ...virtualPolicy, canCreate: false } } });
  act(() => expect(result.current.tryAdd('virtual-new-input', { x: 0, y: 0 })).toBe(true));
  expect(result.current.dialog).toBeNull();
  expect(value.updateModel).not.toHaveBeenCalled();
});

it('preserves a new owner creation command scheduled during the owner-switch commit', () => {
  const first = args();
  const second = args();
  const { result, rerender } = renderHook(
    ({ options }) => {
      const action = useVirtualEndpointCreation(options);
      const previousOwner = useRef(first.owner);
      useLayoutEffect(() => {
        if (previousOwner.current !== options.owner) {
          previousOwner.current = options.owner;
          action.tryAdd('virtual-new-input', { x: 4, y: 8 });
        }
      }, [options.owner, action.tryAdd]);
      return action;
    },
    { initialProps: { options: first } },
  );
  rerender({ options: second });
  expect(dialog(result.current.dialog).initialName).toBe('MixRelay IO');
  expect(first.createDevice).not.toHaveBeenCalled();
  expect(second.createDevice).not.toHaveBeenCalled();
});
it('does not create a directional node when the Driver has no corresponding stream', () => {
  const value = args();
  const settings = {
    ...virtualPolicy,
    devices: virtualPolicy.devices.map((device) => ({
      ...device,
      spec: { ...virtualSpec, capture: null },
    })),
  };
  const { result } = renderHook(() => useVirtualEndpointCreation({ ...value, settings }));
  act(() => result.current.tryAdd(virtualEndpointItemId('input', virtualSpec.id), { x: 0, y: 0 }));
  expect(value.updateModel).not.toHaveBeenCalled();
});
it('reports absent Driver creation support through the dialog while leaving the graph unchanged', async () => {
  const value = args();
  const { result } = renderHook(() =>
    useVirtualEndpointCreation({ ...value, createDevice: undefined }),
  );
  act(() => result.current.tryAdd('virtual-new-input', { x: 0, y: 0 }));
  await act(async () => dialog(result.current.dialog).onCreate('Named'));
  expect(dialog(result.current.dialog).error).toBeTruthy();
  expect(value.updateModel).not.toHaveBeenCalled();
});
it.each(['reject', 'null', 'missing stream'] as const)(
  'reuses an allocated identity after a %s readback and confirms only one node',
  async (outcome) => {
    const value = args();
    if (outcome === 'reject') value.createDevice.mockRejectedValueOnce(new Error('IPC lost'));
    else if (outcome === 'null') value.createDevice.mockResolvedValueOnce(null);
    else value.createDevice.mockImplementationOnce(async (spec) => ({ ...spec, capture: null }));
    const { result } = renderHook(() => useVirtualEndpointCreation(value));
    act(() => result.current.tryAdd('virtual-new-input', { x: 14, y: 22 }));
    await act(async () => dialog(result.current.dialog).onCreate('First attempt'));
    expect(dialog(result.current.dialog).error).toBeTruthy();
    await act(async () => dialog(result.current.dialog).onCreate('Confirmed name'));
    expect(value.createDevice).toHaveBeenCalledTimes(2);
    expect(value.createDevice.mock.calls[1]?.[0].id).toBe(value.createDevice.mock.calls[0]?.[0].id);
    expect(value.updateModel).toHaveBeenCalledTimes(1);
    expect(result.current.dialog).toBeNull();
  },
);
it('serializes repeated confirmations and ignores a late readback after unmount', async () => {
  const value = args();
  const pending = Promise.withResolvers<VirtualEndpointSpec | null>();
  value.createDevice.mockReturnValue(pending.promise);
  const { result, unmount } = renderHook(() => useVirtualEndpointCreation(value));
  act(() => result.current.tryAdd('virtual-new-input', { x: 0, y: 0 }));
  act(() => dialog(result.current.dialog).onCreate('Named'));
  act(() => dialog(result.current.dialog).onCreate('Duplicate'));
  expect(value.createDevice).toHaveBeenCalledTimes(1);
  unmount();
  await act(async () => pending.resolve(virtualSpec));
  expect(value.updateModel).not.toHaveBeenCalled();
});
it('ignores readback when creation permission is revoked during the native operation', async () => {
  const value = args();
  const pending = Promise.withResolvers<VirtualEndpointSpec | null>();
  value.createDevice.mockReturnValue(pending.promise);
  const { result, rerender } = renderHook(({ options }) => useVirtualEndpointCreation(options), {
    initialProps: { options: value },
  });
  act(() => result.current.tryAdd('virtual-new-input', { x: 0, y: 0 }));
  act(() => dialog(result.current.dialog).onCreate('Named'));
  rerender({ options: { ...value, settings: { ...virtualPolicy, canCreate: false } } });
  await act(async () => pending.resolve(virtualSpec));
  expect(value.updateModel).not.toHaveBeenCalled();
  expect(result.current.dialog).toBeNull();
});

it('does not allocate a device identity when the native creation template disappears before confirmation', async () => {
  const value = args();
  const { result, rerender } = renderHook(({ options }) => useVirtualEndpointCreation(options), {
    initialProps: { options: value },
  });
  act(() => result.current.tryAdd('virtual-new-input', { x: 0, y: 0 }));
  rerender({ options: { ...value, settings: { ...virtualPolicy, creationTemplate: null } } });
  await act(async () => dialog(result.current.dialog).onCreate('Unavailable template'));
  expect(value.createDevice).not.toHaveBeenCalled();
});
it('does not add the completed device to an unloaded graph document', async () => {
  const value = args();
  const pending = Promise.withResolvers<VirtualEndpointSpec | null>();
  value.createDevice.mockReturnValue(pending.promise);
  const { result, rerender } = renderHook(
    ({ options }: { options: Parameters<typeof useVirtualEndpointCreation>[0] }) =>
      useVirtualEndpointCreation(options),
    { initialProps: { options: value as Parameters<typeof useVirtualEndpointCreation>[0] } },
  );
  act(() => result.current.tryAdd('virtual-new-input', { x: 0, y: 0 }));
  act(() => dialog(result.current.dialog).onCreate('Named'));
  rerender({ options: { ...value, model: null } });
  await act(async () => pending.resolve(virtualSpec));
  expect(value.updateModel).not.toHaveBeenCalled();
  expect(result.current.dialog).toBeNull();
});

it('consumes virtual selections while the graph or native device settings are unavailable', () => {
  const value = args();
  const { result, rerender } = renderHook(
    ({ options }: { options: Parameters<typeof useVirtualEndpointCreation>[0] }) =>
      useVirtualEndpointCreation(options),
    { initialProps: { options: value as Parameters<typeof useVirtualEndpointCreation>[0] } },
  );
  rerender({ options: { ...value, model: null } });
  act(() => expect(result.current.tryAdd('virtual-new-input', { x: 0, y: 0 })).toBe(true));
  rerender({ options: { ...value, settings: undefined } });
  act(() => expect(result.current.tryAdd('virtual-new-input', { x: 0, y: 0 })).toBe(true));
  expect(value.updateModel).not.toHaveBeenCalled();
  expect(result.current.dialog).toBeNull();
});

it('closes a pending creation dialog when its Driver owner is replaced', () => {
  const value = args();
  const { result, rerender } = renderHook(({ options }) => useVirtualEndpointCreation(options), {
    initialProps: { options: value },
  });
  act(() => result.current.tryAdd('virtual-new-input', { x: 0, y: 0 }));
  expect(result.current.dialog).not.toBeNull();
  rerender({ options: { ...value, owner: args().owner } });
  expect(result.current.dialog).toBeNull();
  expect(value.createDevice).not.toHaveBeenCalled();
});
