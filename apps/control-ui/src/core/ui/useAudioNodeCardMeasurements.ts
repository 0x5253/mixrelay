import { useCallback, useEffect, useLayoutEffect, useRef } from 'react';
import type { AudioNodeCardProps } from './AudioNodeCard';
import { useElementBounds } from './useElementBounds';

function cssHeight(value: string) {
  const pixels = Number.parseFloat(value);
  return Number.isFinite(pixels) ? pixels : null;
}

function borderBoxHeight(element: HTMLElement) {
  const style = getComputedStyle(element);
  const height = cssHeight(style.height);
  if (height === null) return null;
  if (style.boxSizing === 'border-box') return height;
  return (
    height +
    (cssHeight(style.paddingTop) ?? 0) +
    (cssHeight(style.paddingBottom) ?? 0) +
    (cssHeight(style.borderTopWidth) ?? 0) +
    (cssHeight(style.borderBottomWidth) ?? 0)
  );
}

/** Face and controls measurements reported to the graph that owns node geometry. */
export function useCardMeasurements({
  onFaceHeightChange,
  onContentHeightChange,
}: Pick<AudioNodeCardProps, 'onFaceHeightChange' | 'onContentHeightChange'>) {
  const [faceRef, faceBounds] = useElementBounds<HTMLElement>();
  const [contentRef, contentBounds] = useElementBounds<HTMLDivElement>();
  const controlsNode = useRef<HTMLDivElement | null>(null);
  const measureRef = useCallback(
    (element: HTMLDivElement | null) => {
      controlsNode.current = element;
      contentRef(element);
    },
    [contentRef],
  );
  const measureControls = useCallback(() => {
    const controls = controlsNode.current;
    if (!controls) return;
    const total = borderBoxHeight(controls);
    const panel = controls.querySelector<HTMLElement>('[data-node-disclosure]');
    const disclosure = panel ? borderBoxHeight(panel) : 0;
    if (total === null || disclosure === null || disclosure > total) return;
    onContentHeightChange?.(total, disclosure);
  }, [onContentHeightChange]);
  useEffect(() => {
    if (faceBounds.height > 0) onFaceHeightChange?.(faceBounds.height);
  }, [faceBounds.height, onFaceHeightChange]);
  useLayoutEffect(() => {
    // ResizeObserver values trigger a coherent DOM read; they are not mixed as measurements.
    void contentBounds.height;
    void contentBounds.width;
    measureControls();
  }, [contentBounds.height, contentBounds.width, measureControls]);
  return { faceRef, faceBounds, contentRef: measureRef, measureControls };
}
