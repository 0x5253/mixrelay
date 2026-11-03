import { expect, it } from 'vitest';
import { protocolFixtures } from '../../../test-support/protocolFixtures';
import { ControlPortError } from '../../domain/control/errors';
import {
  graphApplyResponse,
  graphCatalog,
  mixRelayStatus,
  runtimeGraphDocument,
} from './protocolDecoders';
import { decodeReply, INVALID_RESPONSE_STATUS, refusal } from './responses';

function rejection(run: () => unknown) {
  try {
    run();
  } catch (error) {
    return error instanceof ControlPortError ? [error.status, error.body] : error;
  }
  return 'accepted';
}

it('reports a malformed reply as an uncertain server failure naming the broken field', () => {
  expect(rejection(() => decodeReply(graphApplyResponse, { revision: 3 }, 'apply'))).toEqual([
    INVALID_RESPONSE_STATUS,
    {
      code: 'control_request_failed',
      parameters: { reason: 'invalid_response', path: 'apply.protocolVersion' },
    },
  ]);
  expect(INVALID_RESPONSE_STATUS).toBeGreaterThanOrEqual(500);
});

it('refuses a reply from another protocol version or graph schema', () => {
  const status = protocolFixtures.status();
  expect(
    rejection(() => decodeReply(mixRelayStatus, { ...status, protocolVersion: 2 }, 's')),
  ).toMatchObject([502, { parameters: { path: 's.protocolVersion' } }]);
  const document = { schemaVersion: 3, revision: 1, nodes: [], edges: [] };
  expect(rejection(() => decodeReply(runtimeGraphDocument, document, 'g'))).toMatchObject([
    502,
    { parameters: { path: 'g.schemaVersion' } },
  ]);
});

it('keeps a valid reply, including engine fields this UI does not know', () => {
  const catalog = { ...protocolFixtures.catalog(), layoutRevision: 4, futureField: true };
  expect(decodeReply(graphCatalog, catalog, 'catalog')).toEqual(catalog);
  const status = { ...protocolFixtures.status(), sampleFaults: { nonFinite: 0 } };
  expect(decodeReply(mixRelayStatus, status, 'status')).toEqual(status);
});

it('decodes nested graph edges and refuses an unknown channel mapping', () => {
  const edge = {
    id: 'e',
    source: { nodeId: 'a', portId: 'out' },
    destination: { nodeId: 'b', portId: 'in' },
    channelMapping: { mode: 'explicit', routes: [{ sourceChannel: 0, destinationChannel: 1 }] },
  };
  const document = { schemaVersion: 2, revision: 1, nodes: [], edges: [edge] };
  expect(decodeReply(runtimeGraphDocument, document, 'g')).toEqual(document);
  const unknown = { ...document, edges: [{ ...edge, channelMapping: { mode: 'matrix' } }] };
  expect(rejection(() => decodeReply(runtimeGraphDocument, unknown, 'g'))).toMatchObject([
    502,
    { parameters: { path: 'g.edges[0].channelMapping.mode' } },
  ]);
});

it('keeps the status and code of a refusal, including codes this UI does not name', () => {
  expect(refusal(409, { code: 'graph_revision_stale', parameters: { revision: 4 } })).toEqual(
    new ControlPortError(409, { code: 'graph_revision_stale', parameters: { revision: 4 } }),
  );
  const desktop = refusal(402, { code: 'graph_catalog_unavailable' });
  expect([desktop.status, desktop.body.code, desktop.body.parameters]).toEqual([
    402,
    'graph_catalog_unavailable',
    undefined,
  ]);
});

it('still reports the status of a refusal whose body is unreadable', () => {
  for (const body of [null, 'Bad Gateway', { code: 7 }, []])
    expect(refusal(503, body)).toEqual(
      new ControlPortError(503, {
        code: 'control_request_failed',
        parameters: { reason: 'invalid_error_body' },
      }),
    );
});

it('rethrows anything that is not a decoding failure', () => {
  const broken = () => {
    throw new TypeError('bug');
  };
  expect(() => decodeReply(broken, {}, 'x')).toThrow(TypeError);
});
