import { describe, expect, it, vi } from 'vitest';

import { AudioMeterStore, audioMeterKey, emptyAudioMeterLevels } from './AudioMeterStore';

function snapshot(peaks: number[], present = true) {
  return {
    protocolVersion: 1 as const,
    revision: 1,
    nodes: present ? [{ nodeId: 'app/a', portId: 'out:b', peaks }] : [],
  };
}

describe('meter telemetry ownership', () => {
  it('notifies subscribers only when the level changes and removes unsubscribed listeners independently', () => {
    const store = new AudioMeterStore();
    const key = audioMeterKey('app/a', 'out:b');
    expect(key).toBe('app%2Fa:out%3Ab');
    expect(store.levels(key)).toBe(emptyAudioMeterLevels());
    const first = vi.fn();
    const second = vi.fn();
    const unsubscribeFirst = store.subscribe(key, first);
    const unsubscribeSecond = store.subscribe(key, second);
    store.update(snapshot([1, 0.001]));
    expect(store.levels(key)).toEqual([100, 0]);
    store.update(snapshot([1, 0.001]));
    expect(first).toHaveBeenCalledOnce();
    unsubscribeFirst();
    store.update(snapshot([0.1, 0.01]));
    expect(first).toHaveBeenCalledOnce();
    expect(second).toHaveBeenCalledTimes(2);
    unsubscribeSecond();
    store.clear();
    expect(second).toHaveBeenCalledTimes(2);
    expect(store.levels(key)).toBe(emptyAudioMeterLevels());
  });

  it('resets invalid peaks and removed meters rather than retaining an old positive signal', () => {
    const store = new AudioMeterStore();
    const key = audioMeterKey('app/a', 'out:b');
    const listener = vi.fn();
    store.subscribe(key, listener);
    store.update(snapshot([1]));
    store.update(snapshot([Number.NaN, Number.POSITIVE_INFINITY, -1, 0]));
    expect(store.levels(key)).toEqual([0, 0, 0, 0]);
    store.update(snapshot([], false));
    expect(store.levels(key)).toBe(emptyAudioMeterLevels());
    expect(listener).toHaveBeenCalledTimes(3);
    store.clear();
    expect(listener).toHaveBeenCalledTimes(3);
  });
});
