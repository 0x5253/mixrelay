import assert from 'node:assert/strict';
import { appendFileSync, copyFileSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { basename, join, resolve } from 'node:path';
import { parseArgs } from 'node:util';
import { pathToFileURL } from 'node:url';
import { createCandidate } from '../qualification.mjs';
import { loadContract, qualificationMatrix } from '../support-contract.mjs';

export function selectCandidate(inputs, output, targetIds = []) {
  const run = JSON.parse(readFileSync(join(inputs, 'build-run.json')));
  assert.equal(run.repository?.full_name, '0x5253/mixrelay');
  assert.equal(run.path, '.github/workflows/build-linux-release.yml');
  assert.equal(run.conclusion, 'success', 'Linux package build must succeed');
  assert.match(run.head_sha, /^[a-f0-9]{40}$/);
  const source = readFileSync(join(inputs, 'macos/source-revision.txt'), 'utf8').trim();
  assert.equal(source, run.head_sha, 'macOS and Linux must use the same source commit');
  for (const path of readdirSync(join(inputs, 'linux'), { recursive: true }).filter(p => basename(p) === 'source-revision.txt')) {
    assert.equal(readFileSync(join(inputs, 'linux', path), 'utf8').trim(), source, 'Linux artifact source mismatch');
  }
  const { contract } = loadContract();
  const matrix = qualificationMatrix(contract);
  assert.equal(new Set(targetIds).size, targetIds.length, 'Duplicate target selection');
  assert.ok(targetIds.every(id => matrix.include.some(target => target.id === id)), 'Unknown or unavailable target selection');
  if (targetIds.length) matrix.include = matrix.include.filter(target => targetIds.includes(target.id));
  const selected = contract.artifacts.map(definition => {
    const root = join(inputs, definition.os === 'macos' ? 'macos' : 'linux');
    const suffix = definition.format === 'arch' ? '.pkg.tar.zst' : `.${definition.format}`;
    const files = readdirSync(root, { recursive: true }).filter(path => path.endsWith(suffix)
      && (definition.os === 'macos' || path.startsWith(`candidate-${definition.id}/`)));
    assert.equal(files.length, 1, `Expected exactly one ${definition.id}`);
    const file = basename(files[0]);
    copyFileSync(join(root, files[0]), join(output, file));
    return `${definition.id}:${file}`;
  });
  const version = JSON.parse(readFileSync(new URL('../../package.json', import.meta.url))).version;
  const candidate = createCandidate(output, selected, source, version);
  writeFileSync(join(output, 'mix-relay.candidate.json'), JSON.stringify(candidate, null, 2) + '\n', { flag: 'wx' });
  writeFileSync(join(output, 'selection.json'), JSON.stringify({ linuxBuildRun: run.id, sourceRevision: source,
    suiteRevision: process.env.GITHUB_SHA, targets: matrix.include.map(target => target.id),
    purpose: 'installed candidate diagnostics; does not authorize publication' }, null, 2) + '\n');
  return matrix;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { values } = parseArgs({ options: { inputs: { type: 'string' }, output: { type: 'string' } } });
  const matrix = selectCandidate(values.inputs, values.output, (process.env.TARGET_IDS ?? '').split(',').map(id => id.trim()).filter(Boolean));
  assert.ok(process.env.GITHUB_OUTPUT);
  appendFileSync(process.env.GITHUB_OUTPUT, `matrix=${JSON.stringify(matrix)}\n`);
}
