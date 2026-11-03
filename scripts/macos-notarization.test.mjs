import assert from 'node:assert/strict';
import test from 'node:test';
import { checkNotarizationCredentials, notarizationArguments } from './macos-notarization.mjs';

const KEY = { apiKeyPath: '/fixture/AuthKey_ABCDE12345.p8', keyId: 'ABCDE12345', issuer: '00000000-0000-4000-8000-000000000000' };

test('the API key is checked against Apple before a build, with a bounded wait', () => {
  const calls = [];
  checkNotarizationCredentials(notarizationArguments(KEY), {
    run: (program, args, options) => { calls.push({ program, args, timeout: options.timeout }); return { status: 0, stdout: '{}' }; },
  });
  assert.deepEqual(calls.map(call => [call.program, ...call.args.slice(0, 2)]), [['/usr/bin/xcrun', 'notarytool', 'history']]);
  assert.ok(calls[0].args.includes('--key') && !calls[0].args.includes('--keychain-profile'), 'no keychain item is consulted');
  assert.ok(calls[0].timeout > 0, 'a stalled check cannot hold the release');
});

test('a rejected, stalled or unreachable API key stops the release before building', () => {
  for (const response of [{ status: 1 }, { status: null, signal: 'SIGTERM' }, { status: null, error: new Error('spawn') }]) {
    assert.throws(() => checkNotarizationCredentials(notarizationArguments(KEY), { run: () => response }), /notarization API key/);
  }
});

test('a skipped notarization makes no Apple request', () => {
  checkNotarizationCredentials(notarizationArguments(undefined, true), { run: () => assert.fail('no request when skipped') });
});

test('only API key references are accepted, so no keychain prompt can open', () => {
  assert.throws(() => notarizationArguments({ keychainProfile: 'profile' }), /apiKeyPath/);
});
