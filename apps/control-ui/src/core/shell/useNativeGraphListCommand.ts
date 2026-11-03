import { useContext, useEffect, useRef } from 'react';
import { removableSubscription } from '../application/subscription';
import { NativeShellContext } from './NativeShellContext';

/**
 * Binds View → Show/Hide Graph List (⌃⌘S): the native command toggles the list, and the
 * list's visibility keeps the command label naming what it will do next.
 */
export function useNativeGraphListCommand(visible: boolean, onToggle: () => void) {
  const shell = useContext(NativeShellContext);
  const toggle = useRef(onToggle);
  toggle.current = onToggle;
  useEffect(() => {
    if (!shell) return;
    return removableSubscription(
      shell.onGraphListToggle(() => toggle.current()),
      (error) => console.error('Graph list menu listener failed', error),
    );
  }, [shell]);
  useEffect(() => {
    void shell
      ?.setGraphListVisible(visible)
      .catch((error: unknown) => console.error('Graph list menu label failed', error));
  }, [shell, visible]);
}
