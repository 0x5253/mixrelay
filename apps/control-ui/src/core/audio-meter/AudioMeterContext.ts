import { createContext } from 'react';

import type { AudioMeterStore } from './AudioMeterStore';

export const AudioMeterContext = createContext<AudioMeterStore | null>(null);
