import { useContext, useEffect } from 'react';
import { NativeShellContext } from '../shell/NativeShellContext';
import { useTheme } from './MixRelayProvider';

/** Keeps the native window chrome on the chosen appearance; system follows the OS. */
export function NativeWindowTheme() {
  const shell = useContext(NativeShellContext);
  const { preference } = useTheme();
  useEffect(() => {
    void shell?.setWindowTheme(preference).catch((error: unknown) => {
      console.error('Could not update the native window appearance', error);
    });
  }, [shell, preference]);
  return null;
}
