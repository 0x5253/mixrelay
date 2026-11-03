import assert from 'node:assert/strict';
import test from 'node:test';
import { prepareMacosUi } from './prepare-macos-ui.mjs';

const dns = { status: 1, stderr: 'curl: (6) Could not resolve host: ghcr.io' };

test('XcodeGen DNS loss is retried while the actual installed tool remains required', async () => {
  const calls = [], waits = [];
  const results = [{ status: null }, dns, { status: 0 }, { status: 0 }];
  await prepareMacosUi({ command(program, args) { calls.push([program, args]); return results.shift(); },
    wait: async duration => waits.push(duration) });
  assert.deepEqual(calls, [['xcodegen', ['--version']], ['brew', ['install', 'xcodegen']],
    ['brew', ['install', 'xcodegen']], ['xcodegen', ['--version']]]);
  assert.deepEqual(waits, [2000]);
});

test('persistent DNS failure, other installation errors and a broken executable still fail', async () => {
  for (const results of [[{ status: null }, dns, dns, dns],
    [{ status: null }, { status: 1, stderr: 'formula does not exist' }],
    [{ status: null }, { status: 0 }, { status: 1 }]]) {
    await assert.rejects(prepareMacosUi({ command: () => results.shift(), wait: async () => {} }),
      /XcodeGen/);
    assert.equal(results.length, 0);
  }
});

test('an executable XcodeGen needs no network install', async () => {
  let calls = 0;
  await prepareMacosUi({ command(program) { calls++; assert.equal(program, 'xcodegen'); return { status: 0 }; } });
  assert.equal(calls, 1);
});
