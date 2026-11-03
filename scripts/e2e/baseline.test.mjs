import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { digest, loadContract } from '../support-contract.mjs';
import { selectBaseline, validateBaseline } from './baseline.mjs';

function fixture(t, missing = []) {
  const root = mkdtempSync(join(tmpdir(), 'mixrelay-baseline-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const inputs = join(root, 'inputs'),
    output = join(root, 'baseline');
  mkdirSync(join(inputs, 'macos'), { recursive: true });
  mkdirSync(join(inputs, 'linux'));
  const sourceRevision = 'b'.repeat(40),
    candidateSource = 'a'.repeat(40);
  const candidate = { sourceRevision: candidateSource, version: '0.1.0', artifacts: [] };
  const comparison = {
    status: 'ahead',
    merge_base_commit: { sha: sourceRevision },
    html_url: `https://github.com/0x5253/mixrelay/compare/${sourceRevision}...${candidateSource}`,
  };
  const run = {
    id: 12,
    repository: { full_name: '0x5253/mixrelay' },
    path: '.github/workflows/build-linux-release.yml',
    conclusion: 'success',
    head_sha: sourceRevision,
  };
  writeFileSync(join(inputs, 'build-run.json'), JSON.stringify(run));
  writeFileSync(join(inputs, 'macos/source-revision.txt'), sourceRevision);
  for (const definition of loadContract().contract.artifacts) {
    const root = join(
      inputs,
      definition.os === 'macos' ? 'macos' : `linux/candidate-${definition.id}`,
    );
    mkdirSync(root, { recursive: true });
    if (definition.os === 'linux') writeFileSync(join(root, 'source-revision.txt'), sourceRevision);
    if (missing.includes(definition.id)) continue;
    const suffix = definition.format === 'arch' ? 'pkg.tar.zst' : definition.format;
    writeFileSync(join(root, `${definition.id}-0.1.0.${suffix}`), `previous ${definition.id}`);
    candidate.artifacts.push({ id: definition.id, sha256: digest(`candidate ${definition.id}`) });
  }
  return { root, inputs, output, candidate, comparison, run };
}

test('pins any earlier same-version candidate and exposes all 12 hosted target mappings', (t) => {
  const f = fixture(t);
  const matrix = selectBaseline(f.inputs, f.output, f.candidate, f.comparison);
  assert.equal(matrix.include.length, 12);
  assert.ok(matrix.include.every((target) => target.baseline === 'available'));
  const baseline = JSON.parse(readFileSync(join(f.output, 'baseline.json')));
  assert.equal(baseline.version, '0.1.0');
  assert.equal(baseline.linuxBuildRun, 12);
  for (const artifact of baseline.artifacts)
    validateBaseline(baseline, f.candidate, f.output, artifact.id);
  writeFileSync(join(f.output, baseline.artifacts[0].file), 'changed');
  assert.throws(() => validateBaseline(baseline, f.candidate, f.output, baseline.artifacts[0].id));
});

test('a missing format remains in the matrix and fails its affected target explicitly', (t) => {
  const f = fixture(t, ['linux-x64-arch']);
  const matrix = selectBaseline(f.inputs, f.output, f.candidate, f.comparison);
  assert.equal(matrix.include.length, 12);
  assert.match(
    matrix.include.find((target) => target.id === 'arch-rolling-x64').baseline,
    /MISSING.*linux-x64-arch/,
  );
  const baseline = JSON.parse(readFileSync(join(f.output, 'baseline.json')));
  assert.throws(
    () => validateBaseline(baseline, f.candidate, f.output, 'linux-x64-arch'),
    /Missing baseline artifact/,
  );
});

test('rejects identical sources, identical bytes, later versions and unrelated ancestry', (t) => {
  for (const mutate of [
    (f) => {
      f.candidate.sourceRevision = f.run.head_sha;
    },
    (f) => {
      f.comparison.status = 'diverged';
    },
    (f) => {
      f.comparison.merge_base_commit.sha = 'c'.repeat(40);
    },
    (f) => {
      f.comparison.html_url = 'https://github.com/0x5253/mixrelay/compare/other';
    },
    (f) => {
      f.candidate.version = '0.0.9';
    },
    (f) => {
      f.candidate.artifacts[0].sha256 = digest(`previous ${f.candidate.artifacts[0].id}`);
    },
  ]) {
    const f = fixture(t);
    mutate(f);
    assert.throws(() => selectBaseline(f.inputs, f.output, f.candidate, f.comparison));
  }
});

test('rejects mismatched source records and unsuccessful or unrelated baseline workflows', (t) => {
  for (const mutate of [
    (f) => {
      f.run.conclusion = 'failure';
    },
    (f) => {
      f.run.path = 'other.yml';
    },
    (f) => {
      f.run.repository.full_name = 'other/repo';
    },
    (f) => {
      writeFileSync(join(f.inputs, 'macos/source-revision.txt'), 'c'.repeat(40));
    },
    (f) => {
      writeFileSync(
        join(f.inputs, 'linux/candidate-linux-arm64-deb/source-revision.txt'),
        'c'.repeat(40),
      );
    },
  ]) {
    const f = fixture(t);
    mutate(f);
    writeFileSync(join(f.inputs, 'build-run.json'), JSON.stringify(f.run));
    assert.throws(() => selectBaseline(f.inputs, f.output, f.candidate, f.comparison));
  }
});

test('the workflow CLI emits the selected matrix and missing-baseline summary without skipping the job', async (t) => {
  const { execFileSync } = await import('node:child_process');
  const { fileURLToPath } = await import('node:url');
  const f = fixture(t, ['linux-x64-arch']);
  const candidate = join(f.root, 'candidate.json'),
    comparison = join(f.root, 'comparison.json');
  const output = join(f.root, 'outputs'),
    summary = join(f.root, 'summary');
  writeFileSync(candidate, JSON.stringify(f.candidate));
  writeFileSync(comparison, JSON.stringify(f.comparison));
  execFileSync(
    process.execPath,
    [
      fileURLToPath(new URL('./baseline.mjs', import.meta.url)),
      '--inputs',
      f.inputs,
      '--output',
      f.output,
      '--candidate',
      candidate,
      '--comparison',
      comparison,
    ],
    {
      env: {
        ...process.env,
        TARGET_IDS: 'arch-rolling-x64',
        GITHUB_OUTPUT: output,
        GITHUB_STEP_SUMMARY: summary,
      },
    },
  );
  const matrix = JSON.parse(readFileSync(output, 'utf8').trim().slice('matrix='.length));
  assert.equal(matrix.include.length, 1);
  assert.equal(matrix.include[0].id, 'arch-rolling-x64');
  assert.match(matrix.include[0].baseline, /MISSING baseline artifact/);
  assert.match(readFileSync(summary, 'utf8'), /arch-rolling-x64: MISSING baseline artifact/);
});

test('ambiguous installers and inconsistent package versions fail baseline selection', (t) => {
  const ambiguous = fixture(t);
  writeFileSync(join(ambiguous.inputs, 'macos/another-0.1.0.pkg'), 'ambiguous');
  assert.throws(
    () =>
      selectBaseline(ambiguous.inputs, ambiguous.output, ambiguous.candidate, ambiguous.comparison),
    /Ambiguous/,
  );
  const mixed = fixture(t);
  const directory = join(mixed.inputs, 'linux/candidate-linux-arm64-deb');
  rmSync(join(directory, 'linux-arm64-deb-0.1.0.deb'));
  writeFileSync(join(directory, 'linux-arm64-deb-0.0.9.deb'), 'other version');
  assert.throws(
    () => selectBaseline(mixed.inputs, mixed.output, mixed.candidate, mixed.comparison),
    /versions must match/,
  );
});
