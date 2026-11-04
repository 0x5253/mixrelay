import type { AudioGraphModel } from '../../core/audio-graph/model';
import type { ProcessorParameterChange } from '../../core/domain/control/graph';
import { directString } from './domain/configValues';

/** Called only for explicit processor edits/Undo, never for a structural snapshot replay. */
export function changedProcessorParameters(
  before: AudioGraphModel,
  after: AudioGraphModel,
): ProcessorParameterChange[] {
  const beforeNodes = new Map(before.nodes.map((node) => [node.id, node]));
  return after.nodes.flatMap((node) => {
    const previous = beforeNodes.get(node.id);
    const typeId = directString(node.runtime?.config, 'typeId');
    if (
      node.runtime?.type !== 'provider' ||
      previous?.runtime?.type !== 'provider' ||
      !typeId ||
      directString(previous.runtime.config, 'typeId') !== typeId
    )
      return [];
    const oldControls = new Map(
      previous.appearance.parameterControls?.map((control) => [control.id, control.value]) ?? [],
    );
    return (node.appearance.parameterControls ?? []).flatMap((control) => {
      if (!control.id.startsWith('external:') || oldControls.get(control.id) === control.value)
        return [];
      const parameterId = Number(control.id.slice(9));
      return Number.isInteger(parameterId) && parameterId >= 0
        ? [{ nodeId: node.id, typeId, parameterId, value: control.value }]
        : [];
    });
  });
}
