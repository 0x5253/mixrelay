import { expect, it } from 'vitest';
import { ControlPortError, hasErrorCode, protocolErrorCodes } from './errors';

const refusal = (code: string) => new ControlPortError(409, { code, parameters: null });

it('matches a refusal by any of the given known codes', () => {
  expect(hasErrorCode(refusal('graph_revision_stale'), 'graph_revision_stale')).toBe(true);
  expect(hasErrorCode(refusal('node_not_found'), 'graph_revision_stale', 'node_not_found')).toBe(
    true,
  );
  expect(hasErrorCode(refusal('node_not_found'), 'graph_revision_stale')).toBe(false);
});

it('keeps codes the UI does not name as refusals that match nothing', () => {
  expect(hasErrorCode(refusal('pairing_required'), ...protocolErrorCodes)).toBe(false);
  expect(refusal('pairing_required').message).toBe('pairing_required');
});

it('matches only engine and desktop refusals', () => {
  expect(hasErrorCode(new Error('graph_revision_stale'), 'graph_revision_stale')).toBe(false);
  expect(hasErrorCode({ body: { code: 'graph_revision_stale' } }, 'graph_revision_stale')).toBe(
    false,
  );
});
