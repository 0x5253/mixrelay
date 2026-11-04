import { useEffect } from 'react';
import { hasPlatformModifier, isNativeTextEntryTarget } from '../../core/ui/platformShortcut';

function runGraphHistoryShortcut(event: KeyboardEvent, undo: () => boolean, redo: () => boolean) {
  const key = event.key.toLowerCase();
  if (key === 'z') return event.shiftKey ? redo() : undo();
  return event.ctrlKey && key === 'y' ? redo() : false;
}

/** Window-wide graph Undo/Redo that leaves text entry its own shortcuts. */
export function useGraphHistoryShortcuts(
  undo: () => boolean,
  redo: () => boolean,
  enabled: boolean,
) {
  useEffect(() => {
    if (!enabled) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (
        event.defaultPrevented ||
        event.altKey ||
        !hasPlatformModifier(event) ||
        isNativeTextEntryTarget(event.target)
      ) {
        return;
      }
      if (runGraphHistoryShortcut(event, undo, redo)) event.preventDefault();
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [redo, undo, enabled]);
}
