import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync, existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { packageReleaseArtifacts } from './release-macos.mjs';

test('macOS release builds the plugin before PKG packaging and ships its exact bytes alongside it', async t => {
  const directory = mkdtempSync(join(tmpdir(), 'mixrelay-release-unit-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const output = join(directory, 'release');
  const stage = { payload: { output: join(directory, 'staging/payload') }, packages: { output, version: '1.2.3' } };
  const file = 'MixRelay-StreamDeck-1.2.3.streamDeckPlugin';
  const calls = [];
  const owned = async (_program, args) => {
    calls.push(args);
    if (args[0].endsWith('streamdeck-artifact.mjs')) {
      mkdirSync(args[3], { recursive: true });
      writeFileSync(join(args[3], file), 'validated packed plugin');
    } else {
      assert.ok(args[0].endsWith('package-macos-release.mjs'));
      assert.equal(existsSync(output), false, 'PKG packaging keeps its empty output contract');
      mkdirSync(output);
      writeFileSync(join(output, 'MixRelay.pkg'), 'final notarised PKG');
    }
    return { status: 0 };
  };
  await packageReleaseArtifacts(owned, stage, {});
  assert.equal(calls.length, 2);
  assert.equal(readFileSync(join(output, file), 'utf8'), 'validated packed plugin');
  await assert.rejects(packageReleaseArtifacts(async () => ({ status: 2 }), stage, {}), /failed/);
  let call = 0;
  await assert.rejects(packageReleaseArtifacts(async () => ({ status: call++ === 0 ? 0 : 1 }), stage, {}));
  assert.equal(readFileSync(join(output, file), 'utf8'), 'validated packed plugin');
});
