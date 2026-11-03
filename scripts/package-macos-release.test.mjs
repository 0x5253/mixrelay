import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { preparePayload } from './prepare-macos-release-payload.mjs';

const stage = fileURLToPath(new URL('./package-macos-release.mjs', import.meta.url));

test('the release packaging stage reports a failure as the in-process release did', () => {
  // Rejected before any file or command is touched.
  const payload = { manager: 'relative/MixRelay.app', hal: '/h', driver: '/d', engine: '/e', uninstaller: '/u', output: '/o' };
  const expected = (() => { try { preparePayload(payload); } catch (error) { return error.message; } })();
  assert.ok(expected);
  const result = spawnSync(process.execPath, [stage, JSON.stringify({ payload, packages: { skipNotarization: true } })], { encoding: 'utf8' });
  assert.equal(result.status, 1);
  assert.equal(result.stdout, '');
  assert.equal(result.stderr, `${expected}\n`);
});

test('the release packaging stage rejects missing notarization references before preparing or signing', () => {
  const result = spawnSync(process.execPath, [stage, JSON.stringify({ payload: {}, packages: {} })], { encoding: 'utf8' });
  assert.equal(result.status, 1);
  assert.equal(result.stdout, '');
  assert.match(result.stderr, /^Signing config requires notarization\.apiKeyPath/);
});
