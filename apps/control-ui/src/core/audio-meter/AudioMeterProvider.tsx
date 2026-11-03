import type { PropsWithChildren } from 'react';

import { AudioMeterContext } from './AudioMeterContext';
import type { AudioMeterStore } from './AudioMeterStore';

export function AudioMeterProvider({
  children,
  store,
}: PropsWithChildren<{ store: AudioMeterStore | null }>) {
  return <AudioMeterContext.Provider value={store}>{children}</AudioMeterContext.Provider>;
}
