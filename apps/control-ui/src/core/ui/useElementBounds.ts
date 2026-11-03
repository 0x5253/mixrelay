import { useCallback, useLayoutEffect, useState } from 'react';

/** Measurement stays in Core geometry and is independent of the UI primitive library. */
export function useElementBounds<T extends HTMLElement>() {
  const [element, setElement] = useState<T | null>(null);
  const [bounds, setBounds] = useState({ width: 0, height: 0 });
  const ref = useCallback((next: T | null) => setElement(next), []);
  useLayoutEffect(() => {
    if (!element) return;
    const observer = new ResizeObserver(([entry]) => {
      if (!entry) return;
      const box = entry.borderBoxSize[0];
      const width = box?.inlineSize ?? entry.contentRect.width;
      const height = box?.blockSize ?? entry.contentRect.height;
      // Repeated observer deliveries are geometry signals, not new layout state.
      setBounds((previous) =>
        previous.width === width && previous.height === height ? previous : { width, height },
      );
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, [element]);
  return [ref, bounds] as const;
}
