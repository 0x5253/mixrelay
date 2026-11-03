import type { AudioGraphChangeKind } from '../../core/audio-graph/AudioGraphCanvas';
import type { AudioGraphModel, AudioGraphNodeAppearance } from '../../core/audio-graph/model';

function changedControl(
  before: AudioGraphNodeAppearance,
  after: AudioGraphNodeAppearance,
  builtin: boolean,
) {
  const changed: string[] = [];
  if (before.gainControl?.valueDb !== after.gainControl?.valueDb) changed.push('gain');
  const previous = new Map(before.parameterControls?.map((control) => [control.id, control.value]));
  for (const control of after.parameterControls ?? []) {
    if (builtin && control.id === 'preset') {
      // Preset matching is derived from parameter values, including a match while dragging.
      // Explicit preset selection is a separate runtime command, not a level gesture.
      continue;
    }
    if (previous.get(control.id) !== control.value) changed.push(`parameter:${control.id}`);
  }
  return changed.length === 1 ? changed[0] : undefined;
}

function changedBand(before: AudioGraphNodeAppearance, after: AudioGraphNodeAppearance) {
  const previous = new Map(before.parameterControls?.map((control) => [control.id, control.value]));
  const changed =
    after.parameterControls?.filter((control) => previous.get(control.id) !== control.value) ?? [];
  const band = after.equalizer?.bands.find(
    (band) =>
      changed.length > 0 &&
      changed.every((control) => control.id === band.frequencyId || control.id === band.gainId),
  );
  return band?.id;
}

export function graphHistoryGroup(
  before: AudioGraphModel,
  after: AudioGraphModel,
  kind: AudioGraphChangeKind,
) {
  const pairs = before.nodes.flatMap((previous, index) => {
    const next = after.nodes[index];
    return next ? [{ previous, next }] : [];
  });
  if (
    pairs.length !== before.nodes.length ||
    pairs.length !== after.nodes.length ||
    pairs.some(({ previous, next }) => previous.id !== next.id)
  )
    return undefined;
  const [change, ...others] = pairs.filter(({ previous, next }) => previous !== next);
  if (!change || others.length || before.edges !== after.edges) return undefined;
  const { previous, next } = change;
  if (kind === 'runtime' && next.runtime?.type === 'effect') return undefined;
  const group = `${kind}:${next.id}`;
  if (kind === 'parameter_group') {
    const band = changedBand(previous.appearance, next.appearance);
    return band ? `${group}:${band}` : undefined;
  }
  if (kind !== 'level' && kind !== 'processor_parameter') return group;
  // One node can contain many controls. Threshold and ratio are separate gestures,
  // even when the second starts within the first control's debounce window.
  const control = changedControl(
    previous.appearance,
    next.appearance,
    next.runtime?.type === 'effect',
  );
  return control ? `${group}:${control}` : undefined;
}
