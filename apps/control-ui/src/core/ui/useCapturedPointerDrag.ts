import { type PointerEvent, useCallback, useEffect, useRef } from 'react';

/** Pointer-captured vertical drag. Capture is released on pointer up/cancel, loss of
 * capture, disabling and unmount; no window listeners or timers are created. */
export function useCapturedPointerDrag<State>({
  disabled,
  begin,
  update,
}: {
  disabled?: boolean;
  /** Return undefined to ignore the press. */
  begin: (event: PointerEvent<HTMLDivElement>) => State | undefined;
  update: (state: State, event: PointerEvent<HTMLDivElement>) => void;
}) {
  const drag = useRef<{ pointer: number; element: HTMLDivElement; state: State } | null>(null);
  const finish = useCallback(() => {
    const active = drag.current;
    drag.current = null;
    if (active?.element.hasPointerCapture?.(active.pointer))
      active.element.releasePointerCapture(active.pointer);
  }, []);
  useEffect(() => {
    if (disabled) finish();
    return finish;
  }, [disabled, finish]);
  const end = (event: PointerEvent<HTMLDivElement>) => {
    if (drag.current?.pointer === event.pointerId) finish();
  };
  return {
    onPointerDown(event: PointerEvent<HTMLDivElement>) {
      if (disabled || event.button !== 0 || drag.current) return;
      const state = begin(event);
      if (state === undefined) return;
      event.currentTarget.setPointerCapture(event.pointerId);
      drag.current = { pointer: event.pointerId, element: event.currentTarget, state };
    },
    onPointerMove(event: PointerEvent<HTMLDivElement>) {
      const active = drag.current;
      if (active?.pointer === event.pointerId) update(active.state, event);
    },
    onPointerUp: end,
    onPointerCancel: end,
    onLostPointerCapture: finish,
  };
}
