import { useContext } from 'react';
import { useTranslation } from 'react-i18next';
import { type MidiAction, sameTarget, shortControl } from '../domain/midi';
import { MidiContext, MidiGraphContext, MidiNodeContext } from './MidiContext';

/** Replace an audio action with one mapping focus owner while map mode is active. */
export function useMidiControl(action: MidiAction) {
  const midi = useContext(MidiContext);
  const graph = useContext(MidiGraphContext);
  const node = useContext(MidiNodeContext);
  const { t } = useTranslation();
  if (!midi || !graph || !node) return null;
  const target = { graphId: graph.graphId, nodeId: node.nodeId };
  const assignment = {
    target,
    action,
    label: node.nodeName,
    graphName: graph.graphName,
    nodeName: node.nodeName,
  };
  const bindings =
    midi.status?.bindings.filter(
      (binding) => sameTarget(binding.target, target) && binding.action === action,
    ) ?? [];
  const selected = Boolean(
    midi.selected && sameTarget(midi.selected.target, target) && midi.selected.action === action,
  );
  return {
    active: midi.active,
    label: t('midi.mapTarget', { name: node.nodeName, action: t(`midi.actions.${action}`) }),
    text: bindings.map((binding) => shortControl(binding.control, t('midi.kinds.note'))).join(', '),
    selected,
    select: () => midi.assign(assignment),
    keyDown: (event: {
      key: string;
      preventDefault: () => void;
      continuePropagation: () => void;
    }) => {
      if ((event.key === 'Delete' || event.key === 'Backspace') && bindings[0]) {
        event.preventDefault();
        midi.remove(bindings[0]);
      } else {
        event.continuePropagation();
      }
    },
  };
}
