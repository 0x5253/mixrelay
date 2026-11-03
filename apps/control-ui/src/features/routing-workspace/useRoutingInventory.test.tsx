import { act, cleanup, renderHook } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type {
  AudioDeviceDescriptor,
  CapturableApplication,
} from '../../core/domain/control/inventory';

import {
  DEVICE_PRESENCE_POLL_MS,
  INVENTORY_POLL_MS,
  useRoutingInventory,
} from './useRoutingInventory';

const application: CapturableApplication = {
  bundleId: 'org.mozilla.firefox',
  displayName: 'Firefox',
  iconDataUrl: null,
  processId: 123,
  selector: { match: 'bundle_id', bundle_id: 'org.mozilla.firefox', include_descendants: false },
};
const device: AudioDeviceDescriptor = {
  displayName: 'Interface',
  inputChannels: 64,
  outputChannels: 16,
  isDefaultInput: false,
  isDefaultOutput: false,
  uid: 'interface',
};

function port() {
  return {
    listCapturableApplications: vi
      .fn<() => Promise<CapturableApplication[]>>()
      .mockResolvedValue([]),
    listAudioDevices: vi.fn<() => Promise<AudioDeviceDescriptor[]>>().mockResolvedValue([]),
  };
}

async function settle() {
  await act(async () => {});
}

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe('routing inventory independent owners', () => {
  it('publishes device removal and reconnection while application discovery stays pending', async () => {
    vi.useFakeTimers();
    const source = port();
    const applications = Promise.withResolvers<CapturableApplication[]>();
    source.listCapturableApplications.mockReturnValue(applications.promise);
    source.listAudioDevices.mockResolvedValue([device]);
    const { result, unmount } = renderHook(() => useRoutingInventory(source, true));
    await settle();
    expect(result.current.audioDevices).toEqual([device]);
    expect(result.current.devicesObserved).toBe(true);
    expect(result.current.refreshing).toBe(true);
    source.listAudioDevices.mockResolvedValue([]);
    await act(async () => vi.advanceTimersByTimeAsync(DEVICE_PRESENCE_POLL_MS));
    expect(result.current.audioDevices).toEqual([]);
    source.listAudioDevices.mockResolvedValue([device]);
    await act(async () => vi.advanceTimersByTimeAsync(DEVICE_PRESENCE_POLL_MS));
    expect(result.current.audioDevices).toEqual([device]);
    expect(source.listAudioDevices).toHaveBeenCalledTimes(3);
    await act(async () => vi.advanceTimersByTimeAsync(INVENTORY_POLL_MS));
    expect(source.listCapturableApplications).toHaveBeenCalledTimes(1);
    expect(result.current.failed).toBe(false);
    unmount();
    await act(async () => applications.resolve([application]));
    expect(result.current.applications).toEqual([]);
  });

  it('does not clear application errors when independent device polling succeeds', async () => {
    vi.useFakeTimers();
    const source = port();
    source.listCapturableApplications.mockRejectedValue(new Error('Icon search failed'));
    const { result } = renderHook(() => useRoutingInventory(source, false));
    await settle();
    expect(result.current.failed).toBe(true);
    source.listAudioDevices.mockResolvedValue([device]);
    await act(async () => vi.advanceTimersByTimeAsync(DEVICE_PRESENCE_POLL_MS));
    expect(result.current.audioDevices).toEqual([device]);
    expect(result.current.failed).toBe(true);
    expect(source.listCapturableApplications).toHaveBeenCalledTimes(1);
    source.listCapturableApplications.mockResolvedValue([application]);
    act(() => result.current.refresh());
    await settle();
    expect(result.current.failed).toBe(false);
  });
});

describe('routing inventory lifecycle', () => {
  it('refreshes newly launched apps and both device directions on menu open and focus', async () => {
    const controlPort = port();
    const { result, rerender } = renderHook(
      ({ opened }) => useRoutingInventory(controlPort, opened),
      {
        initialProps: { opened: false },
      },
    );
    await settle();
    controlPort.listCapturableApplications.mockResolvedValue([application]);
    controlPort.listAudioDevices.mockResolvedValue([device]);
    rerender({ opened: true });
    await settle();
    expect(result.current.applications).toEqual([application]);
    expect(result.current.audioDevices).toEqual([device]);
    controlPort.listAudioDevices.mockResolvedValue([]);
    act(() => window.dispatchEvent(new Event('focus')));
    await settle();
    expect(result.current.audioDevices).toEqual([]);
  });

  it('preserves successful lists on independent failure and exposes retry recovery', async () => {
    const controlPort = port();
    controlPort.listCapturableApplications.mockResolvedValue([application]);
    const { result } = renderHook(() => useRoutingInventory(controlPort, false));
    await settle();
    controlPort.listCapturableApplications.mockRejectedValue(new Error('native discovery failed'));
    controlPort.listAudioDevices.mockResolvedValue([device]);
    act(() => result.current.refresh());
    await settle();
    expect(result.current.failed).toBe(true);
    expect(result.current.applications).toEqual([application]);
    expect(result.current.audioDevices).toEqual([device]);
    controlPort.listCapturableApplications.mockResolvedValue([]);
    controlPort.listAudioDevices.mockRejectedValue(new Error('device discovery failed'));
    act(() => result.current.refresh());
    await settle();
    expect(result.current.applications).toEqual([]);
    expect(result.current.audioDevices).toEqual([device]);
    expect(result.current.failed).toBe(true);
    controlPort.listAudioDevices.mockResolvedValue([]);
    act(() => result.current.refresh());
    await settle();
    expect(result.current.failed).toBe(false);
  });

  it('serializes requests and keeps only device presence polling after the menu closes', async () => {
    vi.useFakeTimers();
    const controlPort = port();
    const pending = Promise.withResolvers<AudioDeviceDescriptor[]>();
    controlPort.listAudioDevices.mockReturnValueOnce(pending.promise);
    const { result, rerender } = renderHook(
      ({ opened }) => useRoutingInventory(controlPort, opened),
      {
        initialProps: { opened: true },
      },
    );
    await settle();
    act(() => {
      result.current.refresh();
      window.dispatchEvent(new Event('focus'));
      vi.advanceTimersByTime(INVENTORY_POLL_MS * 3);
    });
    await settle();
    expect(controlPort.listAudioDevices).toHaveBeenCalledTimes(1);
    expect(result.current.refreshing).toBe(true);
    await act(async () => pending.resolve([device]));
    await act(async () => vi.advanceTimersByTimeAsync(DEVICE_PRESENCE_POLL_MS));
    expect(controlPort.listAudioDevices).toHaveBeenCalledTimes(2);
    rerender({ opened: false });
    const applicationReads = controlPort.listCapturableApplications.mock.calls.length;
    await act(async () => vi.advanceTimersByTimeAsync(DEVICE_PRESENCE_POLL_MS * 3));
    expect(controlPort.listAudioDevices).toHaveBeenCalledTimes(5);
    expect(controlPort.listCapturableApplications).toHaveBeenCalledTimes(applicationReads);
  });
});

describe('routing inventory cancellation', () => {
  it('ignores stale responses across control port replacement and unmount', async () => {
    vi.useFakeTimers();
    const oldPort = port();
    const pending = Promise.withResolvers<AudioDeviceDescriptor[]>();
    oldPort.listAudioDevices.mockReturnValue(pending.promise);
    const nextPort = port();
    nextPort.listAudioDevices.mockResolvedValue([device]);
    const { result, rerender, unmount } = renderHook(
      ({ source }) => useRoutingInventory(source, true),
      {
        initialProps: { source: oldPort },
      },
    );
    await settle();
    rerender({ source: nextPort });
    await settle();
    await act(async () => pending.resolve([]));
    expect(result.current.audioDevices).toEqual([device]);
    const late = Promise.withResolvers<AudioDeviceDescriptor[]>();
    nextPort.listAudioDevices.mockReturnValue(late.promise);
    act(() => result.current.refresh());
    await settle();
    unmount();
    await act(async () => late.resolve([]));
    act(() => {
      result.current.refresh();
      window.dispatchEvent(new Event('focus'));
      vi.advanceTimersByTime(INVENTORY_POLL_MS * 3);
    });
    expect(nextPort.listAudioDevices).toHaveBeenCalledTimes(2);
    expect(result.current.audioDevices).toEqual([device]);
  });

  it('pauses hidden-window discovery and refreshes on becoming visible', async () => {
    const visibility = vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('hidden');
    const controlPort = port();
    renderHook(() => useRoutingInventory(controlPort, true));
    await settle();
    expect(controlPort.listAudioDevices).not.toHaveBeenCalled();
    visibility.mockReturnValue('visible');
    act(() => document.dispatchEvent(new Event('visibilitychange')));
    await settle();
    expect(controlPort.listAudioDevices).toHaveBeenCalledTimes(1);
  });

  it('treats absent optional discovery APIs as empty supported inventories', async () => {
    const source = {};
    const { result } = renderHook(() => useRoutingInventory(source, false));
    await settle();
    expect(result.current.applications).toEqual([]);
    expect(result.current.audioDevices).toEqual([]);
    expect(result.current.failed).toBe(false);
    expect(result.current.refreshing).toBe(false);
    expect(result.current.devicesObserved).toBe(false);
  });

  it('ignores a response received while hidden and resumes presence checks when visible', async () => {
    vi.useFakeTimers();
    const visibility = vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible');
    const source = port();
    const pending = Promise.withResolvers<AudioDeviceDescriptor[]>();
    source.listAudioDevices.mockReturnValueOnce(pending.promise);
    const { result } = renderHook(() => useRoutingInventory(source, false));
    await settle();
    visibility.mockReturnValue('hidden');
    await act(async () => pending.resolve([device]));
    await act(async () => vi.advanceTimersByTimeAsync(DEVICE_PRESENCE_POLL_MS * 3));
    expect(source.listAudioDevices).toHaveBeenCalledTimes(1);
    expect(result.current.devicesObserved).toBe(false);
    expect(result.current.refreshing).toBe(false);
    visibility.mockReturnValue('visible');
    act(() => document.dispatchEvent(new Event('visibilitychange')));
    await settle();
    expect(result.current.devicesObserved).toBe(true);
    expect(result.current.audioDevices).toEqual([]);
  });
});
