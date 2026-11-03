import { useContext, useSyncExternalStore } from 'react';
import { AudioMeterContext } from './AudioMeterContext';
import { audioMeterKey } from './AudioMeterStore';

const unavailable = () => null;

export function useDynamicsMeterReading(nodeId: string, portId: string) {
  const store = useContext(AudioMeterContext);
  const key = audioMeterKey(nodeId, portId);
  return useSyncExternalStore(
    (listener) => (store ? store.subscribe(key, listener) : () => undefined),
    () => store?.dynamics(key) ?? null,
    unavailable,
  );
}
