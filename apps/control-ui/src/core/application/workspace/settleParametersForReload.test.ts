import { expect, it } from 'vitest';
import { ControlPortError } from '../../domain/control/errors';
import { settleParametersForReload } from './settleParametersForReload';

it('allows a completed parameter refusal to be replaced by an explicit graph reload', async () => {
  const refused = new ControlPortError(409, { code: 'graph_revision_stale', parameters: {} });
  await expect(settleParametersForReload(() => Promise.reject(refused))).resolves.toBeUndefined();
  await expect(settleParametersForReload(() => Promise.resolve())).resolves.toBeUndefined();
});

it('propagates an unexpected parameter owner failure before reload can discard the draft', async () => {
  const failure = new Error('parameter owner failed');
  await expect(settleParametersForReload(() => Promise.reject(failure))).rejects.toBe(failure);
});
