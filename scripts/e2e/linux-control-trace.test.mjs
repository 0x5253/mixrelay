import assert from 'node:assert/strict';
import test from 'node:test';
import { controlTraceEvent, controlTraceStatuses } from './linux-control-trace.mjs';

test('socket send status counts distinguish an unobserved transport from successful replies without saving contents', () => {
  const sent = String.raw`sendto(17, "{\"body\":{\"ok\":{\"status\":503,\"body\":{\"code\":\"control_unavailable\"}}},\"status\":200}\n", 100, MSG_NOSIGNAL, NULL, 0) = 100`;
  assert.deepEqual(controlTraceStatuses(sent), [503, 200]);
  assert.equal(controlTraceEvent(sent).code, 'control_unavailable');
  assert.deepEqual(controlTraceStatuses('strace: Process 42 attached'), []);
  assert.deepEqual(controlTraceStatuses(String.raw`sendto(17, "{\"status\":206,\"body\":\"private\"}", 50, MSG_NOSIGNAL, NULL, 0) = 50`), [206]);
});

test('trace distinguishes confirmed admission refusal from an uncertain response without retaining payloads', () => {
  const refusal = String.raw`writev(17, [{iov_base="{\"body\":{\"ok\":{\"status\":503,\"body\":{\"code\":\"control_unavailable\",\"parameters\":{\"operationOutcome\":\"not_applied\",\"reason\":\"private value\"}}}},\"status\":200,\"id\":12}\n"}], 1) = 200`;
  assert.deepEqual(controlTraceEvent(refusal), { code: 'control_unavailable', statuses: [503, 200],
    outcomes: { operationOutcome: 'not_applied' } });
  assert.deepEqual(controlTraceEvent(refusal.replace('not_applied', 'uncertain'))?.outcomes,
    { operationOutcome: 'uncertain' });
  assert.equal(JSON.stringify(controlTraceEvent(refusal)).includes('private value'), false);
});

test('ordinary telemetry, request bodies and arbitrary transport diagnostics are discarded', () => {
  for (const line of ['write(1, "private key", 11) = 11', 'strace: Process 42 attached',
    String.raw`write(18, "{\"status\":200,\"body\":{\"key\":\"private\"}}", 50) = 50`,
    '{"status":503,"body":{"code":"private_value"}}', '{"code":"control_unavailable"}']) {
    assert.equal(controlTraceEvent(line), null);
  }
});
