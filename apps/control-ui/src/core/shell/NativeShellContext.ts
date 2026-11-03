import { createContext } from 'react';
import type { NativeShellPort } from '../ports/NativeShellPort';

/** The desktop shell chosen by the composition root; null in a browser. */
export const NativeShellContext = createContext<NativeShellPort | null>(null);
