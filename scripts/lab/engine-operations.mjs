// Bounded, idempotent lab exchanges with the engine's asynchronous operation API.
import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';

// Keep retries below the server's 60-second completed-receipt retention.
const OPERATION_BUDGET_MS = 45000;
// Native input can spend 120 seconds awaiting first-use consent plus readiness.
const PREPARATION_BUDGET_MS = 130000;
const engineHost = { host: 'mixrelay-engine' };

export function engineOperations(io, prefix = 'lab') {
  async function operation(method, path, body) {
    const id = `${prefix}-${randomUUID()}`;
    const receipt = `/api/v1/operations/${id}`;
    const headers = { ...engineHost, 'x-mixrelay-operation-request': id,
      ...(body === undefined ? {} : { 'content-type': 'application/json' }) };
    let deadline = io.now() + OPERATION_BUDGET_MS;
    const preparationDeadline = io.now() + PREPARATION_BUDGET_MS;
    let pending = false;
    while (io.now() < deadline) {
      let reply;
      try {
        reply = pending
          ? await io.request(receipt, { headers: engineHost, timeoutMs: 5000 })
          : await io.request(path, { method, headers, body, timeoutMs: 5000 });
      } catch {
        await io.delay(250); // Retry the same ID after a lost request or result.
        continue;
      }
      const pendingId = reply.headers['x-mixrelay-operation'];
      const resultId = reply.headers['x-mixrelay-operation-result'];
      assert.ok(pendingId === undefined || pendingId === id, `${method} ${path}: foreign pending operation ID`);
      assert.ok(resultId === undefined || resultId === id, `${method} ${path}: foreign result operation ID`);
      if (pendingId !== undefined) {
        assert.equal(reply.status, 202, `${method} ${path}: pending operation must return HTTP 202`);
        assert.equal(resultId, undefined, `${method} ${path}: operation cannot be pending and complete`);
        pending = true;
        // Ownership alone does not extend a slow or hung engine operation.
        try {
          const status = await io.request('/api/v1/graph/status', { headers: engineHost, timeoutMs: 5000 });
          const reason = JSON.parse(status.body)?.pendingReason;
          if (reason === 'awaiting_microphone_permission')
            deadline = Math.min(preparationDeadline, io.now() + OPERATION_BUDGET_MS);
        } catch {
          // Missing permission observations keep the existing deadline.
        }
        await io.delay(250);
        continue;
      }
      let parsed;
      try {
        parsed = reply.body ? JSON.parse(reply.body) : undefined;
      } catch {
        parsed = undefined;
      }
      assert.notEqual(parsed?.parameters?.operationOutcome, 'uncertain',
        `${method} ${path} has no confirmed result (operation ${id}); check the graph before editing it`);
      if (resultId === id) {
        // Acknowledging reclaims bounded receipt storage after the final result.
        await io.request(receipt, { method: 'DELETE', headers: engineHost, timeoutMs: 5000 }).catch(() => {});
      }
      return { ...reply, json: parsed };
    }
    assert.fail(`${method} ${path} did not finish (operation ${id}); check the graph before editing it`);
  }
  async function api(path, body) {
    const reply = await operation(body === undefined ? 'GET' : 'PUT', `/api/v1/${path}`,
      body === undefined ? undefined : JSON.stringify(body));
    assert.ok(reply.status >= 200 && reply.status < 300, `/api/v1/${path}: HTTP ${reply.status} ${reply.body}`);
    return reply.json;
  }
  return { operation, api };
}
