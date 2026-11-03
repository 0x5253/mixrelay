import { act, renderHook } from '@testing-library/react';
import { renderToString } from 'react-dom/server';
import { describe, expect, it } from 'vitest';

import { AudioMeterContext } from './AudioMeterContext';
import { AudioMeterStore } from './AudioMeterStore';
import { useAudioMeterLevels } from './useAudioMeterLevels';

describe('node meter subscription boundaries', () => {
  it('returns empty levels without a store or a complete port identity', () => {
    const store = new AudioMeterStore();
    const { result: outside } = renderHook(() => useAudioMeterLevels('source', 'out'));
    expect(outside.current).toEqual([]);
    const { result, rerender } = renderHook(
      ({ nodeId, portId }) => useAudioMeterLevels(nodeId, portId),
      {
        initialProps: { nodeId: '', portId: 'out' },
        wrapper: ({ children }) => (
          <AudioMeterContext.Provider value={store}>{children}</AudioMeterContext.Provider>
        ),
      },
    );
    expect(result.current).toEqual([]);
    rerender({ nodeId: 'source', portId: '' });
    expect(result.current).toEqual([]);
  });

  it('updates the selected meter and unsubscribes when the node identity changes', () => {
    const store = new AudioMeterStore();
    const { result, rerender } = renderHook(({ nodeId }) => useAudioMeterLevels(nodeId, 'out'), {
      initialProps: { nodeId: 'source' },
      wrapper: ({ children }) => (
        <AudioMeterContext.Provider value={store}>{children}</AudioMeterContext.Provider>
      ),
    });
    act(() =>
      store.update({
        protocolVersion: 1,
        revision: 1,
        nodes: [{ nodeId: 'source', portId: 'out', peaks: [1] }],
      }),
    );
    expect(result.current).toEqual([100]);
    rerender({ nodeId: 'other' });
    expect(result.current).toEqual([]);
  });

  it('uses an empty server snapshot without presenting stale client audio', () => {
    const Meter = () => <span>{useAudioMeterLevels('source', 'out').length}</span>;
    expect(renderToString(<Meter />)).toBe('<span>0</span>');
  });
});
