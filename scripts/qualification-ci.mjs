import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { appendFileSync, copyFileSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { parseArgs } from 'node:util';
import { digest, loadContract, checkGenerated, qualificationMatrix, releaseInstallers } from './support-contract.mjs';
import { validateCandidate, readReports, evaluateQualification } from './qualification.mjs';
import { prepareReleaseManifest, assertQualifiedRelease } from './prepare-release-manifest.mjs';

const execute = (program, args, options = {}) => execFileSync(program, args, { encoding: 'utf8', timeout: 60000, maxBuffer: 16 * 1024 * 1024, ...options }).trim();
const api = path => JSON.parse(execute('gh', ['api', path]));
const read = path => JSON.parse(readFileSync(path));
export function readWorkflowReports(directory) {
  // The Actions bundle also contains publication/mix-relay.qualification.json,
  // which is a receipt, not an environment's scenario report.
  return readReports(join(directory, 'qualification'));
}
export function validateRun(run, receipt) {
  assert.match(String(receipt.runId), /^[1-9]\d*$/);
  assert.equal(String(run.id), receipt.runId);
  assert.equal(String(run.run_attempt), receipt.runAttempt);
  assert.equal(run.path, '.github/workflows/qualify-release.yml');
  assert.equal(run.head_branch, 'main');
  assert.equal(run.event, 'workflow_dispatch');
  assert.equal(run.status, 'completed');
  assert.equal(run.conclusion, 'success');
  assert.equal(run.repository.full_name, '0x5253/mixrelay');
}
export function successfulSourceRun(runs, candidate) {
  const run = runs.find(run => run.head_sha === candidate.sourceRevision && run.head_branch === 'main'
    && run.event === 'workflow_dispatch' && run.status === 'completed' && run.conclusion === 'success'
    && run.path === '.github/workflows/native-lab.yml' && run.repository.full_name === '0x5253/mixrelay');
  assert.ok(run, 'The candidate source commit has no successful main native build/unit-test workflow');
  return run;
}
function sourceCheck(candidate) {
  return successfulSourceRun(api(`repos/0x5253/mixrelay/actions/workflows/native-lab.yml/runs?head_sha=${candidate.sourceRevision}&status=success&per_page=100`).workflow_runs, candidate);
}

async function main() {
  const command = process.argv[2];
  const { values } = parseArgs({ args: process.argv.slice(3), options: {
    directory: { type: 'string' }, reports: { type: 'string' }, output: { type: 'string' }, tag: { type: 'string' },
  } });
  assert.ok(values.directory, '--directory is required');
  const directory = resolve(values.directory);
  const loaded = loadContract();
  checkGenerated(loaded.contract);
  const candidate = validateCandidate(read(join(directory, 'mix-relay.candidate.json')), directory, loaded);
  if (command === 'prepare') {
    assert.equal(process.env.GITHUB_REPOSITORY, '0x5253/mixrelay');
    assert.equal(process.env.GITHUB_REF, 'refs/heads/main');
    execute('git', ['merge-base', '--is-ancestor', candidate.sourceRevision, 'HEAD']);
    sourceCheck(candidate);
    if (process.env.GITHUB_STEP_SUMMARY) appendFileSync(process.env.GITHUB_STEP_SUMMARY,
      `## GitHub-hosted qualification\n\n${loaded.contract.qualificationTargets.map(target => `- ${target.id}: ${target.execution.kind === 'unavailable' ? target.execution.reason : `${target.execution.runner}; not yet tested`}`).join('\n')}\n\nUnavailable targets remain missing in the release gate.\n`);
    appendFileSync(process.env.GITHUB_OUTPUT, `source=${candidate.sourceRevision}\nmatrix=${JSON.stringify(qualificationMatrix(loaded.contract))}\n`);
  } else if (command === 'seal') {
    const sourceRun = sourceCheck(candidate);
    const reports = readReports(values.reports);
    const result = evaluateQualification(candidate, reports, loaded);
    assert.equal(result.passed, true, result.failures.join('\n'));
    assert.ok(reports.every(report => report.provenance.runId === process.env.GITHUB_RUN_ID
      && report.provenance.runAttempt === process.env.GITHUB_RUN_ATTEMPT), 'Evidence belongs to another workflow execution');
    const path = prepareReleaseManifest(['--directory', directory, '--product', candidate.product, '--version', candidate.version,
      '--published-at', new Date().toISOString(), '--candidate', join(directory, 'mix-relay.candidate.json'), '--reports', values.reports,
      ...releaseInstallers(candidate, loaded.contract).flatMap(artifact => {
        const definition = loaded.contract.artifacts.find(item => item.id === artifact.id);
        return ['--artifact', `${definition.os}:${definition.arch}:${artifact.file}`];
      })]);
    mkdirSync(values.output);
    copyFileSync(path, join(values.output, 'mix-relay.release.json'));
    writeFileSync(join(values.output, 'mix-relay.qualification.json'), JSON.stringify({ schemaVersion: 1,
      runId: process.env.GITHUB_RUN_ID, runAttempt: process.env.GITHUB_RUN_ATTEMPT,
      sourceRevision: candidate.sourceRevision, candidateSha256: digest(JSON.stringify(candidate)),
      manifestSha256: digest(readFileSync(path)),
      sourceCheckRunId: String(sourceRun.id), sourceCheckRunAttempt: String(sourceRun.run_attempt),
    }, null, 2) + '\n', { flag: 'wx' });
  } else if (command === 'verify-published') {
    assert.match(values.tag ?? '', /^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/);
    assert.equal(values.tag, `v${candidate.version}`);
    const receipt = read(join(directory, 'mix-relay.qualification.json'));
    assert.equal(receipt.schemaVersion, 1);
    assert.equal(receipt.sourceRevision, candidate.sourceRevision);
    assert.equal(receipt.candidateSha256, digest(JSON.stringify(candidate)));
    assert.equal(receipt.manifestSha256, digest(readFileSync(join(directory, 'mix-relay.release.json'))));
    validateRun(api(`repos/0x5253/mixrelay/actions/runs/${receipt.runId}`), receipt);
    assert.match(receipt.sourceCheckRunId ?? '', /^[1-9]\d*$/);
    const sourceRun = successfulSourceRun([api(`repos/0x5253/mixrelay/actions/runs/${receipt.sourceCheckRunId}`)], candidate);
    assert.equal(String(sourceRun.run_attempt), receipt.sourceCheckRunAttempt);
    execute('git', ['fetch', '--no-tags', 'origin', `refs/tags/${values.tag}`]);
    assert.equal(execute('git', ['rev-parse', 'FETCH_HEAD^{commit}']), candidate.sourceRevision, 'Published tag is not the qualified source');
    execute('git', ['merge-base', '--is-ancestor', candidate.sourceRevision, 'HEAD']);
    execute('gh', ['run', 'download', receipt.runId, '--repo', '0x5253/mixrelay', '--name', 'qualification-evidence', '--dir', values.output]);
    const reports = readWorkflowReports(values.output);
    assert.ok(reports.every(report => report.provenance.runId === receipt.runId && report.provenance.runAttempt === receipt.runAttempt), 'Evidence belongs to another workflow execution');
    assertQualifiedRelease(read(join(directory, 'mix-relay.release.json')), candidate, directory, reports, loaded);
    console.log('Published bytes match the candidate qualified by the successful native workflow');
  } else throw new Error('Expected prepare, seal or verify-published');
}
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch(error => { console.error(error.message); process.exitCode = 1; });
}
