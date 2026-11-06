import assert from 'node:assert/strict';
import {
  appendFileSync,
  copyFileSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  writeFileSync,
} from 'node:fs';
import { basename, join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { parseArgs } from 'node:util';
import { artifactBytes } from '../qualification.mjs';
import { loadContract, qualificationMatrix } from '../support-contract.mjs';
import { versionAtLeast } from '../core/environment-policy.mjs';

export function validateBaseline(baseline, candidate, directory, artifactId) {
  assert.equal(baseline.schemaVersion, 1);
  assert.match(baseline.sourceRevision, /^[a-f0-9]{40}$/);
  assert.notEqual(
    baseline.sourceRevision,
    candidate.sourceRevision,
    'Baseline source is identical to candidate',
  );
  assert.match(baseline.version, /^\d+\.\d+\.\d+$/);
  assert.ok(
    versionAtLeast(candidate.version, baseline.version),
    'Baseline version is newer than candidate',
  );
  const artifact = baseline.artifacts.find((item) => item.id === artifactId);
  assert.ok(artifact, `Missing baseline artifact: ${artifactId}`);
  assert.notEqual(
    artifact.sha256,
    candidate.artifacts.find((item) => item.id === artifactId)?.sha256,
    `Baseline installer is identical to candidate: ${artifactId}`,
  );
  assert.deepEqual(artifactBytes(directory, artifact), {
    sha256: artifact.sha256,
    size: artifact.size,
  });
  return artifact;
}

export function selectBaseline(inputs, output, candidate, comparison, loaded = loadContract()) {
  const run = JSON.parse(readFileSync(join(inputs, 'build-run.json')));
  assert.equal(run.repository?.full_name, '0x5253/mixrelay');
  assert.equal(run.path, '.github/workflows/build-linux-release.yml');
  assert.equal(run.conclusion, 'success', 'Baseline Linux package build must succeed');
  assert.match(run.head_sha, /^[a-f0-9]{40}$/);
  const sourceRevision = readFileSync(join(inputs, 'macos/source-revision.txt'), 'utf8').trim();
  assert.equal(sourceRevision, run.head_sha, 'Baseline macOS and Linux source mismatch');
  assert.notEqual(
    sourceRevision,
    candidate.sourceRevision,
    'Baseline source is identical to candidate',
  );
  assert.equal(comparison.status, 'ahead', 'Baseline must be an ancestor of candidate');
  assert.equal(
    comparison.merge_base_commit?.sha,
    sourceRevision,
    'Baseline comparison source mismatch',
  );
  assert.equal(
    comparison.html_url,
    `https://github.com/0x5253/mixrelay/compare/${sourceRevision}...${candidate.sourceRevision}`,
    'Candidate comparison source mismatch',
  );
  const linux = readdirSync(join(inputs, 'linux'), { recursive: true });
  for (const file of linux.filter((file) => basename(file) === 'source-revision.txt')) {
    assert.equal(
      readFileSync(join(inputs, 'linux', file), 'utf8').trim(),
      sourceRevision,
      'Baseline Linux artifact source mismatch',
    );
  }
  mkdirSync(output);
  const artifacts = [],
    versions = new Set();
  for (const definition of loaded.contract.artifacts) {
    const root = join(inputs, definition.os === 'macos' ? 'macos' : 'linux');
    const suffix = definition.format === 'arch' ? '.pkg.tar.zst' : `.${definition.format}`;
    const files = readdirSync(root, { recursive: true }).filter(
      (file) =>
        file.endsWith(suffix) &&
        (definition.os === 'macos' || file.startsWith(`candidate-${definition.id}/`)),
    );
    assert.ok(files.length <= 1, `Ambiguous baseline artifact: ${definition.id}`);
    if (!files.length) continue;
    const file = basename(files[0]);
    const version = file.match(/(?:^|[-_])(\d+\.\d+\.\d+)(?=[-_.])/);
    assert.ok(version, `Baseline filename must contain product version: ${file}`);
    versions.add(version[1]);
    copyFileSync(join(root, files[0]), join(output, file));
    artifacts.push({ id: definition.id, file, ...artifactBytes(output, { file }) });
  }
  assert.equal(versions.size, 1, 'Baseline installer versions must match');
  const baseline = {
    schemaVersion: 1,
    sourceRevision,
    version: [...versions][0],
    linuxBuildRun: run.id,
    artifacts,
  };
  for (const artifact of artifacts) validateBaseline(baseline, candidate, output, artifact.id);
  writeFileSync(join(output, 'baseline.json'), `${JSON.stringify(baseline, null, 2)}\n`, {
    flag: 'wx',
  });
  const matrix = qualificationMatrix(loaded.contract);
  matrix.include = matrix.include.map((target) => ({
    ...target,
    baseline: artifacts.some((item) => item.id === target.artifact)
      ? 'available'
      : `MISSING baseline artifact: ${target.artifact}`,
  }));
  return matrix;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { values } = parseArgs({
    options: {
      inputs: { type: 'string' },
      output: { type: 'string' },
      candidate: { type: 'string' },
      comparison: { type: 'string' },
    },
  });
  const matrix = selectBaseline(
    values.inputs,
    values.output,
    JSON.parse(readFileSync(values.candidate)),
    JSON.parse(readFileSync(values.comparison)),
  );
  if (process.env.GITHUB_STEP_SUMMARY) {
    const { sourceRevision } = JSON.parse(readFileSync(join(values.output, 'baseline.json')));
    appendFileSync(
      process.env.GITHUB_STEP_SUMMARY,
      `\n## Upgrade baseline ${sourceRevision}\n\n${matrix.include.map((target) => `- ${target.id}: ${target.baseline}`).join('\n')}\n`,
    );
  }
  const ids = (process.env.TARGET_IDS ?? '')
    .split(',')
    .map((id) => id.trim())
    .filter(Boolean);
  assert.equal(new Set(ids).size, ids.length, 'Duplicate target selection');
  assert.ok(
    ids.every((id) => matrix.include.some((target) => target.id === id)),
    'Unknown target',
  );
  if (ids.length) matrix.include = matrix.include.filter((target) => ids.includes(target.id));
  assert.ok(process.env.GITHUB_OUTPUT);
  appendFileSync(process.env.GITHUB_OUTPUT, `matrix=${JSON.stringify(matrix)}\n`);
}
