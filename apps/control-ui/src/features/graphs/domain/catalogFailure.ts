import { ControlPortError } from '../../../core/domain/control/errors';

export function errorCode(error: unknown): string {
  if (!(error instanceof ControlPortError)) return 'control_request_failed';
  const parameters = error.body.parameters;
  if (
    parameters &&
    typeof parameters === 'object' &&
    'resource' in parameters &&
    parameters.resource === 'graph_catalog'
  ) {
    return 'graph_catalog_unavailable';
  }
  return error.body.code;
}

export function workspaceRejected(error: ControlPortError): boolean {
  const parameters = error.body.parameters;
  return Boolean(
    parameters &&
      typeof parameters === 'object' &&
      'workspaceOutcome' in parameters &&
      parameters.workspaceOutcome === 'not_applied',
  );
}

export function workspaceUncertain(error: unknown) {
  return !(error instanceof ControlPortError) || (error.status >= 500 && !workspaceRejected(error));
}

export function retryableRead(error: unknown) {
  if (!(error instanceof ControlPortError)) return true;
  // A refusal about the catalog resource itself is not retried.
  if (errorCode(error) === 'graph_catalog_unavailable') return false;
  return [408, 429, 502, 503, 504].includes(error.status);
}

export function mutationUncertain(error: unknown) {
  if (!(error instanceof ControlPortError)) return true;
  if (error.status < 500 || workspaceRejected(error)) return false;
  const parameters = error.body.parameters;
  return !(
    parameters &&
    typeof parameters === 'object' &&
    'catalogOutcome' in parameters &&
    parameters.catalogOutcome === 'not_applied'
  );
}
