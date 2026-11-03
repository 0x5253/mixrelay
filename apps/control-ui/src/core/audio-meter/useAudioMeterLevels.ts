import { useContext, useSyncExternalStore } from 'react';

import { AudioMeterContext } from './AudioMeterContext';
import { audioMeterKey, emptyAudioMeterLevels } from './AudioMeterStore';

export function useAudioMeterLevels(nodeId?: string, portId?: string): readonly number[] {
  const store = useContext(AudioMeterContext);
  const key = nodeId && portId ? audioMeterKey(nodeId, portId) : '';
  return useSyncExternalStore(
    (listener) => (store && key ? store.subscribe(key, listener) : () => undefined),
    () => (store && key ? store.levels(key) : emptyAudioMeterLevels()),
    emptyAudioMeterLevels,
  );
}
