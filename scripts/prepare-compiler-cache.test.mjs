import assert from 'node:assert/strict';
import test from 'node:test';
import { prepareCompilerCache } from './prepare-compiler-cache.mjs';

test('healthy remote cache stays enabled', () => {
  const result = prepareCompilerCache(() => ({ status: 0 }), { RUNNER_TEMP: '/qa' });
  assert.equal(result.mode, 'github-actions');
  assert.equal(result.environment.SCCACHE_GHA_ENABLED, undefined);
});
test('remote startup failure is retried with isolated local sccache, preserving the compiler wrapper', () => {
  const calls = [];
  const result = prepareCompilerCache((program, args, options) => {
    calls.push({ program, args, options });
    return { status: calls.length === 1 ? 2 : 0, stderr: 'storage 503 with signed URL' };
  }, { RUNNER_TEMP: '/qa', SCCACHE_GHA_ENABLED: 'true', RUSTC_WRAPPER: 'sccache' });
  assert.equal(result.mode, 'local');
  assert.equal(result.environment.SCCACHE_GHA_ENABLED, 'off');
  assert.equal(calls[2].options.env.RUSTC_WRAPPER, 'sccache');
  assert.equal(calls[2].options.env.SCCACHE_DIR, '/qa/sccache-local');
  assert.deepEqual(calls[1].args, ['--stop-server']);
});
test('local cache or compiler failure is a preflight failure, never a passing build', () => {
  assert.throws(() => prepareCompilerCache(() => ({ status: 1 }), { RUNNER_TEMP: '/qa' }), /both remote and local/);
  assert.throws(() => prepareCompilerCache(() => ({ status: 2 }), {}), /RUNNER_TEMP/);
});
