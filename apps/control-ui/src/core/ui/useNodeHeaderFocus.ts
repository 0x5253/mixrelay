import { type FocusEvent, useLayoutEffect, useRef } from 'react';

function enabledButtons(container: HTMLElement) {
  return [...container.querySelectorAll<HTMLButtonElement>('button')].filter(
    (button) => !button.disabled && button.getAttribute('aria-disabled') !== 'true',
  );
}

/** Conditional recovery commands return focus to the adjacent remaining header action. */
export function useNodeHeaderFocus() {
  const focused = useRef<{
    container: HTMLDivElement;
    element: HTMLButtonElement;
    index: number;
  } | null>(null);
  useLayoutEffect(() => {
    const previous = focused.current;
    if (!previous || previous.element.isConnected) return;
    focused.current = null;
    if (document.activeElement !== document.body) return;
    const buttons = enabledButtons(previous.container);
    (buttons[previous.index] ?? buttons.at(-1))?.focus();
  });
  const onFocusCapture = (event: FocusEvent<HTMLDivElement>) => {
    const element = (event.target as Element).closest<HTMLButtonElement>('button');
    focused.current = element
      ? {
          container: event.currentTarget,
          element,
          index: enabledButtons(event.currentTarget).indexOf(element),
        }
      : null;
  };
  const onBlurCapture = (event: FocusEvent<HTMLDivElement>) => {
    const element = event.target as Element;
    // Disabling a pending trigger may blur it to body before acknowledgement.
    // Retain that owner; meaningful outside focus or an enabled blur releases it.
    const next = event.relatedTarget;
    const intentionalDeparture = next && next !== document.body;
    const retainedEnabledBlur = !element.matches(':disabled') && document.activeElement !== element;
    if (
      element.isConnected &&
      !event.currentTarget.contains(next) &&
      (intentionalDeparture || retainedEnabledBlur)
    )
      focused.current = null;
  };
  return { onFocusCapture, onBlurCapture };
}
