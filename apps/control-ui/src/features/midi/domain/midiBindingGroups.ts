import {
  bindingKey,
  controlKey,
  type MidiAssignment,
  type MidiBinding,
  type MidiControl,
  sameTarget,
} from '../../../core/domain/midi';

/** Mappings grouped by hardware control, in first-seen order. */
export function groupBindings(bindings: readonly MidiBinding[]) {
  const groups = new Map<string, { control: MidiControl; bindings: MidiBinding[] }>();
  for (const binding of bindings) {
    const key = controlKey(binding.control);
    const group = groups.get(key);
    if (group) group.bindings.push(binding);
    else groups.set(key, { control: binding.control, bindings: [binding] });
  }
  return groups;
}

/** A listed mapping is chosen while it is the row the user picked and the session still
 * waits on its target and action. */
export function isChosenBinding(
  binding: MidiBinding,
  chosenKey: string | null,
  selected: MidiAssignment | null,
) {
  return (
    chosenKey === bindingKey(binding) &&
    Boolean(
      selected && sameTarget(selected.target, binding.target) && selected.action === binding.action,
    )
  );
}
