import { ControlPortError, hasErrorCode, type ProtocolErrorCode } from '../control/errors';
import type { ProcessorParameterChange, RuntimeGraphDocument } from '../control/graph';

/** A resolved request is not necessarily an applied edit. Uncertain commands must not be replayed. */
export type NodeControlResult =
  | { state: 'applied'; revision: number }
  | { state: 'rejected'; code: string }
  | { state: 'uncertain'; code: string };

export function failedNodeControl(
  error: unknown,
  sent: boolean,
  accepted: boolean,
): NodeControlResult {
  return {
    state: !sent || (!accepted && definiteNodeRejection(error)) ? 'rejected' : 'uncertain',
    code: errorCode(error),
  };
}

export class GraphApplicationError extends Error {
  constructor(readonly code: string) {
    super(code);
  }
}

export function failure(code: string) {
  return new GraphApplicationError(code);
}

export function errorCode(error: unknown): string {
  if (error instanceof GraphApplicationError) return error.code;
  return error instanceof ControlPortError ? error.body.code : 'control_request_failed';
}

/** Refusals the engine reports before changing anything. */
const definiteRejectionCodes = [
  'graph_revision_stale',
  'engine_replaced',
  'invalid_operation_id',
  'node_disabled',
  'node_not_found',
  'processor_unavailable',
] as const satisfies readonly ProtocolErrorCode[];
/** The same refusals when this queue decides them itself, plus local-only ones: a stale
 * catalog revision and a retry whose failed processor is already gone. */
const definiteLocalRejectionCodes: readonly string[] = [
  ...definiteRejectionCodes,
  'graph_catalog_revision_stale',
  'processor_instance_stale',
];

export function definiteNodeRejection(error: unknown) {
  if (error instanceof ControlPortError) {
    const parameters = error.body.parameters;
    if (
      typeof parameters === 'object' &&
      parameters !== null &&
      (('nodeControlOutcome' in parameters && parameters.nodeControlOutcome === 'not_applied') ||
        // The engine fenced the request before admission (replaced incarnation, bad ID).
        ('operationOutcome' in parameters && parameters.operationOutcome === 'not_admitted'))
    )
      return true;
  }
  return (
    (error instanceof GraphApplicationError && definiteLocalRejectionCodes.includes(error.code)) ||
    hasErrorCode(error, ...definiteRejectionCodes)
  );
}

/**
 * A definite CAS rejection applied nothing, so ordinary concurrent edits are rebased again until
 * the deadline. Any other failure, such as a lost acknowledgement, is rethrown and never replayed.
 */
export function requireRebasableConflict(error: unknown, deadline: number, now: number) {
  if (!hasErrorCode(error, 'graph_revision_stale')) throw error;
  if (now >= deadline) throw failure('graph_conflict_unresolved');
}

export function hasSameProcessor(graph: RuntimeGraphDocument, change: ProcessorParameterChange) {
  return graph.nodes.some((node) => {
    const config = node.config;
    return (
      node.id === change.nodeId &&
      node.type === 'provider' &&
      typeof config === 'object' &&
      config !== null &&
      'typeId' in config &&
      config.typeId === change.typeId
    );
  });
}
