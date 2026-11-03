/** A canvas view: the pan offset in screen pixels and the zoom factor. */
export interface AudioGraphViewport {
  x: number;
  y: number;
  zoom: number;
}

export function sameViewport(a: AudioGraphViewport, b: AudioGraphViewport) {
  return a.x === b.x && a.y === b.y && a.zoom === b.zoom;
}

/** The view to apply for a requested one, or undefined when the canvas already shows it,
 * so acknowledging a saved gesture never animates or repositions a matching view. */
export function viewportToRestore(
  current: AudioGraphViewport,
  requested: AudioGraphViewport | undefined,
): AudioGraphViewport | undefined {
  return requested && !sameViewport(current, requested) ? { ...requested } : undefined;
}

/** The view to save after a move, or undefined when the move only echoes the requested
 * view back. Every other move, including Fit graph, is what the user now sees, so the next
 * mount must restore it rather than an older view where new nodes would land out of sight. */
export function viewportToSave(
  requested: AudioGraphViewport | undefined,
  moved: AudioGraphViewport,
): AudioGraphViewport | undefined {
  return requested && sameViewport(requested, moved) ? undefined : { ...moved };
}
