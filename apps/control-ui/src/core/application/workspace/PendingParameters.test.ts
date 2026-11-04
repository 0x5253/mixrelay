import { expect, it } from 'vitest';
import type { ProcessorParameterChange } from '../../domain/control/graph';
import { PendingParameters } from './PendingParameters';

const change = (parameterId: number, value: number): ProcessorParameterChange => ({
  nodeId: 'eq',
  typeId: 'builtin.eq',
  parameterId,
  value,
});

it('keeps the newest edit per parameter and lets newer edits win over a restored batch', () => {
  const parameters = new PendingParameters();
  parameters.add([change(1, 1), change(1, 2), change(2, 1)]);
  const batch = parameters.take();
  expect([...batch.values()]).toEqual([change(1, 2), change(2, 1)]);
  expect(parameters.take().size).toBe(0);

  parameters.add([change(2, 5)]);
  parameters.restore(batch);
  expect([...parameters.take().values()]).toEqual([change(1, 2), change(2, 5)]);

  parameters.add([change(1, 3)]);
  parameters.clear();
  expect(parameters.take().size).toBe(0);
});
