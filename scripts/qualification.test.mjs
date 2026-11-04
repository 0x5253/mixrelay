import assert from 'node:assert/strict';
import { writeFileSync, readFileSync, symlinkSync, mkdirSync, cpSync } from 'node:fs';
import { join } from 'node:path';
import test from 'node:test';
import { createCandidate, validateCandidate, evaluateQualification, readReports, qualificationInventory } from './qualification.mjs';
import { assertQualifiedRelease } from './prepare-release-manifest.mjs';
import { validateRun, successfulSourceRun, readWorkflowReports } from './qualification-ci.mjs';
import { qualificationMatrix } from './support-contract.mjs';
import { qualificationFixture } from './e2e/qualification.fixture.mjs';

test('one candidate maps the same Universal PKG and per-CPU DEBs across every required environment', t => {
  const f = qualificationFixture(t);
  assert.equal(validateCandidate(f.candidate, f.directory), f.candidate);
  assert.deepEqual(evaluateQualification(f.candidate, readReports(f.reportsPath)), { passed: true, failures: [] });
  assertQualifiedRelease({ product: 'mix-relay', version: '1.2.3', artifacts: f.artifacts }, f.candidate, f.directory, f.reports);
  assert.equal(new Set(f.reports.filter(report => report.targetId.startsWith('macos')).map(report => report.artifactSha256)).size, 1);
});

test('changing an installer, contract or source cannot reuse qualification', t => {
  const f = qualificationFixture(t);
  for (const mutation of [value => { value.contractSha256 = '0'.repeat(64); },
    value => { value.sourceRevision = 'b'.repeat(40); }]) {
    const candidate = structuredClone(f.candidate); mutation(candidate);
    assert.ok(!evaluateQualification(candidate, f.reports).passed);
  }
  writeFileSync(join(f.directory, f.candidate.artifacts[0].file), 'Rebuilt after testing');
  assert.throws(() => validateCandidate(f.candidate, f.directory), /bytes changed/);
});

test('missing environments remain visible and prevent any release manifest', t => {
  const f = qualificationFixture(t);
  const reports = f.reports.slice(1);
  assert.equal(evaluateQualification(f.candidate, reports).passed, false);
  assert.match(qualificationInventory(f.candidate, reports), /macos-15-arm64.*missing report/);
  assert.match(qualificationInventory(f.candidate, reports), /macos-15-arm64 \| not run \| not run/);
  assert.throws(() => assertQualifiedRelease({ product: 'mix-relay', version: '1.2.3', artifacts: f.artifacts }, f.candidate, f.directory, reports), /missing report/);
});

test('wrong host, skipped UI, bad cleanup and mixed CI runs are release failures', t => {
  const f = qualificationFixture(t);
  for (const mutate of [
    r => { r.environment.arch = 'x64'; }, r => { r.environment.kind = 'container'; },
    r => { r.environment.translated = true; }, r => { r.environment.version = '14.1.9'; },
    r => { r.scenarios.find(s => s.id === 'GRAPH-01').status = 'skipped'; },
    r => { r.scenarios.find(s => s.id === 'GRAPH-01').scope = 'installed-api'; },
    r => { r.scenarios[0].evidence = []; }, r => { r.cleanup = 'failed'; },
    r => { r.provenance.runId = '43'; }, r => { r.provenance = {}; },
    r => { r.provenance.runAttempt = '2'; }, r => { delete r.provenance.runAttempt; },
    r => { r.endedAt = '2026-10-22T00:00:00Z'; },
  ]) {
    const reports = structuredClone(f.reports); mutate(reports[0]);
    assert.equal(evaluateQualification(f.candidate, reports).passed, false);
  }
  assert.throws(() => evaluateQualification(f.candidate, [...f.reports, f.reports[0]]), /Duplicate/);
  assert.throws(() => evaluateQualification(f.candidate, [{ targetId: 'invented-os' }]), /Unknown/);
});

test('raw evidence is hashed again rather than trusting a report claim', t => {
  const f = qualificationFixture(t);
  const path = join(f.reportsPath, f.reports[0].targetId, 'unit-evidence.txt');
  writeFileSync(path, 'Changed observation');
  assert.throws(() => readReports(f.reportsPath), /changed raw evidence/);
});

test('publication rechecks environment reports from the Actions bundle without parsing its receipt as a report', t => {
  const f = qualificationFixture(t);
  const bundle = join(f.directory, 'downloaded-actions-bundle'); mkdirSync(bundle);
  cpSync(f.reportsPath, join(bundle, 'qualification'), { recursive: true });
  mkdirSync(join(bundle, 'publication'));
  writeFileSync(join(bundle, 'publication/mix-relay.qualification.json'), JSON.stringify({ schemaVersion: 1, runId: '42' }));
  const reports = readWorkflowReports(bundle);
  assert.equal(reports.length, f.loaded.contract.qualificationTargets.length + f.loaded.contract.artifactChecks.length);
  assert.ok(evaluateQualification(f.candidate, reports).passed);
  writeFileSync(join(bundle, 'qualification', reports[0].targetId, 'unit-evidence.txt'), 'tampered');
  assert.throws(() => readWorkflowReports(bundle), /changed raw evidence/);
});

test('unqualified formats, missing candidate artifacts and symlink installers are rejected', t => {
  const f = qualificationFixture(t);
  const selected = f.candidate.artifacts.map(a => `${a.id}:${a.file}`);
  assert.throws(() => createCandidate(f.directory, selected.slice(1), 'a'.repeat(40), '1.2.3'), /Missing/);
  symlinkSync(join(f.directory, f.candidate.artifacts[0].file), join(f.directory, 'linked.pkg'));
  assert.throws(() => createCandidate(f.directory, ['macos-universal-pkg:linked.pkg', ...selected.slice(1)], 'a'.repeat(40), '1.2.3'), /regular/);
  const manifest = { product: 'mix-relay', version: '1.2.3', artifacts: [...f.artifacts, { os: 'linux', arch: 'x64', file: 'other.rpm' }] };
  assert.throws(() => assertQualifiedRelease(manifest, f.candidate, f.directory, f.reports), /exactly/);
  assert.ok(readFileSync(f.candidatePath).length > 0);
});

test('qualification checks major-version representatives with the product deployment minimum', t => {
  const { loaded } = qualificationFixture(t);
  const matrix = qualificationMatrix(loaded.contract).include;
  assert.equal(matrix.length, 14);
  assert.ok(matrix.every(t => !t.runner.includes('self-hosted')));
  assert.equal(matrix.find(t => t.id === 'macos-15-arm64').runner, 'macos-15');
  assert.equal(matrix.find(t => t.id === 'debian-13-arm64').runner, 'ubuntu-24.04-arm');
  assert.ok(!matrix.some(t => t.os === 'macos' && Number(t.version) < 15));
  assert.ok(!matrix.some(t => t.id.startsWith('macos-15-0')));
  assert.equal(loaded.contract.minimums.macos, '15.0');
  assert.equal(matrix.length, loaded.contract.qualificationTargets.length);
  assert.ok(loaded.contract.qualificationTargets.filter(t => t.os === 'macos').every(t => t.versionMatch === 'major'));
});

test('publication accepts only a successful trusted workflow execution and exact retry attempt', () => {
  const receipt = { runId: '42', runAttempt: '1' };
  const run = { id: 42, run_attempt: 1, path: '.github/workflows/qualify-release.yml', head_branch: 'main',
    event: 'workflow_dispatch', status: 'completed', conclusion: 'success', repository: { full_name: '0x5253/mixrelay' } };
  validateRun(run, receipt);
  for (const changed of [{ conclusion: 'failure' }, { head_branch: 'untrusted' }, { event: 'pull_request' }, { run_attempt: 2 }, { path: 'other.yml' }]) {
    assert.throws(() => validateRun({ ...run, ...changed }, receipt));
  }
});

test('native qualification and publication require successful source CI for the exact main commit', () => {
  const candidate = { sourceRevision: 'a'.repeat(40) };
  const run = { id: 10, head_sha: candidate.sourceRevision, head_branch: 'main', event: 'workflow_dispatch',
    path: '.github/workflows/native-lab.yml', status: 'completed', conclusion: 'success', repository: { full_name: '0x5253/mixrelay' } };
  assert.equal(successfulSourceRun([run], candidate).id, 10);
  for (const changed of [{ head_sha: 'b'.repeat(40) }, { conclusion: 'failure' }, { head_branch: 'feature' },
    { event: 'pull_request' }, { event: 'push' }, { path: '.github/workflows/other.yml' }]) {
    assert.throws(() => successfulSourceRun([{ ...run, ...changed }], candidate), /no successful/);
  }
});
