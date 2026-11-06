import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { selectCandidate } from './select-candidate.mjs';
import { validateCandidate } from '../qualification.mjs';
import { checkVersion } from '../product-version.mjs';

const streamDeckPlugin = `MixRelay-StreamDeck-${checkVersion()}.streamDeckPlugin`;

function fixture(t) {
  const root = mkdtempSync(join(tmpdir(), 'mixrelay-selection-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const inputs = join(root, 'inputs'), output = join(root, 'candidate');
  mkdirSync(join(inputs, 'macos'), { recursive: true }); mkdirSync(output);
  const source = 'a'.repeat(40);
  const run = { repository: { full_name: '0x5253/mixrelay' }, path: '.github/workflows/build-linux-release.yml',
    conclusion: 'success', head_sha: source, id: 1 };
  writeFileSync(join(inputs, 'build-run.json'), JSON.stringify(run));
  writeFileSync(join(inputs, 'macos/source-revision.txt'), source);
  writeFileSync(join(inputs, 'macos/MixRelay.pkg'), 'Synthetic macOS package fixture');
  writeFileSync(join(inputs, `macos/${streamDeckPlugin}`), 'plugin fixture');
  for (const [id, file] of [['linux-x64-deb', 'x64.deb'], ['linux-arm64-deb', 'arm64.deb'], ['linux-x64-arch', 'x64.pkg.tar.zst'],
    ['linux-arm64-arch', 'arm64.pkg.tar.zst'],
    ['linux-x64-rpm', 'x64.rpm'], ['linux-arm64-rpm', 'arm64.rpm']]) {
    const dir = join(inputs, 'linux', `candidate-${id}`); mkdirSync(dir, { recursive: true });
    writeFileSync(join(dir, file), `Synthetic ${id} package fixture`);
    if (id.endsWith('deb')) writeFileSync(join(dir, 'source-revision.txt'), source);
  }
  return { inputs, output, source, run };
}

test('selects the same final bytes and source for every hosted target', t => {
  const { inputs, output, source } = fixture(t);
  const matrix = selectCandidate(inputs, output);
  const candidate = JSON.parse(readFileSync(join(output, 'mix-relay.candidate.json')));
  assert.equal(candidate.sourceRevision, source);
  assert.equal(candidate.artifacts.length, 8);
  validateCandidate(candidate, output);
  assert.equal(matrix.include.length, 15);
  writeFileSync(join(output, 'x64.deb'), 'modified after build');
  assert.throws(() => validateCandidate(candidate, output), /bytes changed/);
});

test('rejects an installer built from a different macOS source', t => {
  const { inputs, output } = fixture(t);
  writeFileSync(join(inputs, 'macos/source-revision.txt'), 'b'.repeat(40));
  assert.throws(() => selectCandidate(inputs, output), /same source commit/);
});

test('selection cannot omit the Stream Deck package from the macOS release', t => {
  const { inputs, output } = fixture(t);
  rmSync(join(inputs, `macos/${streamDeckPlugin}`));
  assert.throws(() => selectCandidate(inputs, output), /Expected exactly one macos-universal-streamdeck/);
});

test('rejects a Linux artifact with a different recorded source', t => {
  const { inputs, output } = fixture(t);
  writeFileSync(join(inputs, 'linux/candidate-linux-arm64-deb/source-revision.txt'), 'c'.repeat(40));
  assert.throws(() => selectCandidate(inputs, output), /Linux artifact source mismatch/);
});

test('rejects an unsuccessful or unrelated package workflow', t => {
  const { inputs, output, run } = fixture(t);
  writeFileSync(join(inputs, 'build-run.json'), JSON.stringify({ ...run, conclusion: 'failure' }));
  assert.throws(() => selectCandidate(inputs, output), /Linux package build must succeed/);
  writeFileSync(join(inputs, 'build-run.json'), JSON.stringify({ ...run, path: '.github/workflows/native-lab.yml' }));
  assert.throws(() => selectCandidate(inputs, output));
});

test('limits diagnosis to explicitly selected hosted targets without accepting unsupported IDs', t => {
  const f = fixture(t);
  assert.deepEqual(selectCandidate(f.inputs, f.output, ['macos-27-arm64']).include.map(target => target.id), ['macos-27-arm64']);
  const invalid = fixture(t);
  assert.throws(() => selectCandidate(invalid.inputs, invalid.output, ['invented-os']), /Unknown or unavailable/);
  assert.throws(() => selectCandidate(invalid.inputs, invalid.output, ['macos-15-0-arm64']), /Unknown or unavailable/);
  assert.throws(() => selectCandidate(invalid.inputs, invalid.output, ['macos-27-arm64', 'macos-27-arm64']), /Duplicate/);
});
