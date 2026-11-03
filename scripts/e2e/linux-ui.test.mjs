import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

test('installed Linux UI selects the visible editor rather than the same-name runtime', () => {
  const result = spawnSync('python3', [fileURLToPath(new URL('./linux-ui.test.py', import.meta.url))], {
    encoding: 'utf8', timeout: 10000,
    env: { ...process.env, PYTHONDONTWRITEBYTECODE: '1' },
  });
  assert.ifError(result.error);
  assert.equal(result.signal, null, result.stderr);
  assert.equal(result.status, 0, result.stderr);
});
