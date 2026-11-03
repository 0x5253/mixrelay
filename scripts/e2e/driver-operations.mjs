import assert from 'node:assert/strict';
import { setTimeout as delay } from 'node:timers/promises';

export function driverOperations({ request, now = Date.now, wait = delay, observe = () => {}, timeoutMs = 45000 }) {
  return async command => {
    const deadline = now() + timeoutMs;
    const exchange = async value => {
      const method = value === undefined ? 'GET' : 'PUT';
      let reply;
      try {
        reply = await request('/api/v1/driver', {
          method,
          body: value === undefined ? undefined : JSON.stringify(value),
          headers: { host: 'mixrelay-driver', ...(value === undefined ? {} : { 'content-type': 'application/json' }) },
          timeoutMs: Math.min(15000, Math.max(1, deadline - now())),
        });
      } catch (error) {
        const action = value?.action ?? 'read';
        observe({ at: now(), action, method, failure: 'transport', code: error.code, message: error.message });
        throw new Error(`Driver ${method} ${action} transport failed (${error.code ?? 'unknown'}): ${error.message}`, { cause: error });
      }
      if (reply.status === 503 && reply.body === 'Driver control ingress is full') {
        observe({ at: now(), action: value?.action ?? 'read', status: reply.status, reason: reply.body });
        return null;
      }
      assert.ok(reply.status >= 200 && reply.status < 300, `Driver ${value ? 'PUT' : 'GET'}: HTTP ${reply.status} ${reply.body}`);
      return JSON.parse(reply.body);
    };
    const read = async () => {
      while (now() < deadline) {
        const result = await exchange();
        if (result) return result;
        await wait(200);
      }
      assert.fail('Driver read admission did not recover before its deadline');
    };
    if (command === undefined) return read();
    while (now() < deadline) {
      const result = await exchange(command);
      if (result) return result;
      const current = await read();
      assert.equal(current.revision, command.expectedRevision, 'Driver changed after an admission rejection');
      await wait(200);
    }
    assert.fail('Driver mutation admission did not recover before its deadline');
  };
}
