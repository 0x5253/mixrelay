import { expect, it } from 'vitest';
import { bindingKey } from '../../../core/domain/midi';
import { midiAssignment, midiBinding } from '../midiFixtures';
import { groupBindings, isChosenBinding } from './midiBindingGroups';

const output = {
  ...midiBinding,
  target: { graphId: 'main', nodeId: 'output' },
  nodeName: 'Programme output',
};
const note = {
  ...midiBinding,
  control: { ...midiBinding.control, kind: 'note' as const, number: 60 },
};

it('groups mappings by hardware control in first-seen order', () => {
  const groups = [...groupBindings([midiBinding, note, output]).values()];
  expect(groups).toEqual([
    { control: midiBinding.control, bindings: [midiBinding, output] },
    { control: note.control, bindings: [note] },
  ]);
});

it('marks a row chosen only while it is the picked row and the session waits on its target and action', () => {
  const picked = bindingKey(midiBinding);
  expect(isChosenBinding(midiBinding, picked, midiAssignment)).toBe(true);
  expect(isChosenBinding(midiBinding, null, midiAssignment)).toBe(false);
  expect(isChosenBinding(midiBinding, picked, null)).toBe(false);
  expect(isChosenBinding(midiBinding, picked, { ...midiAssignment, action: 'mute' })).toBe(false);
  expect(isChosenBinding(midiBinding, picked, { ...midiAssignment, target: output.target })).toBe(
    false,
  );
  expect(isChosenBinding(output, picked, { ...midiAssignment, target: output.target })).toBe(false);
});
