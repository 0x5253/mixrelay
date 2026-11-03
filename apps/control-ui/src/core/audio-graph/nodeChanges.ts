import {
  type AudioGraphModel,
  type AudioGraphNodeSize,
  type AudioGraphPosition,
  moveAudioGraphNode,
  resizeAudioGraphNode,
} from './model';

/** The node changes the canvas reports that matter to the presented layout. */
export type GraphNodeChange =
  | { type: 'position'; id: string; position?: AudioGraphPosition }
  | {
      type: 'dimensions';
      id: string;
      dimensions?: AudioGraphNodeSize;
      /** Set for a user resize; absent for a measurement the canvas made itself. */
      setAttributes?: boolean | 'width' | 'height';
    }
  | { type: 'add' | 'remove' | 'replace' | 'select'; id?: string };

/** The card sizes a user resize asks for. The measured height includes temporary Options
 * content, which is not part of the saved size. */
export function resizedNodeSizes(
  changes: readonly GraphNodeChange[],
  disclosureHeights: ReadonlyMap<string, number>,
) {
  const sizes = new Map<string, AudioGraphNodeSize>();
  for (const change of changes)
    if (change.type === 'dimensions' && change.dimensions && change.setAttributes)
      sizes.set(change.id, {
        width: change.dimensions.width,
        height: change.dimensions.height - (disclosureHeights.get(change.id) ?? 0),
      });
  return sizes;
}

/** Moves and user resizes applied to the presented layout; the audio revision is unchanged. */
export function applyNodeLayoutChanges(
  model: AudioGraphModel,
  changes: readonly GraphNodeChange[],
  disclosureHeights: ReadonlyMap<string, number>,
) {
  const sizes = resizedNodeSizes(changes, disclosureHeights);
  let next = model;
  for (const change of changes) {
    if (change.type === 'position' && change.position)
      next = moveAudioGraphNode(next, change.id, change.position);
    if (change.type !== 'dimensions') continue;
    const size = sizes.get(change.id);
    if (size) next = resizeAudioGraphNode(next, change.id, size);
  }
  return next;
}
