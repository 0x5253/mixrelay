import { readdirSync, readFileSync, lstatSync, writeFileSync } from 'node:fs';
import { basename, resolve, dirname } from 'node:path';
import { pathToFileURL } from 'node:url';
import { parseArgs } from 'node:util';
import { digest, loadContract } from './support-contract.mjs';
import { requiredScenarios } from './core/support-targets.mjs';
import { parseOsRelease, validateEnvironment } from './core/environment-policy.mjs';

export function artifactBytes(directory, artifact) {
  if (basename(artifact.file) !== artifact.file || !artifact.file || /[\\\0]/.test(artifact.file)) throw new Error('Invalid artifact filename');
  const path = resolve(directory, artifact.file);
  if (!lstatSync(path).isFile()) throw new Error(`Artifact is not a regular file: ${artifact.file}`);
  const bytes = readFileSync(path);
  if (!bytes.length) throw new Error(`Empty artifact: ${artifact.file}`);
  return { sha256: digest(bytes), size: bytes.length };
}
function revision(value) {
  if (!/^[a-f0-9]{40}$/.test(value)) throw new Error('A full source commit SHA is required');
}
export function createCandidate(directory, selected, sourceRevision, productVersion, loaded = loadContract()) {
  revision(sourceRevision);
  if (!/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(productVersion)) throw new Error('Invalid candidate version');
  const artifacts = selected.map(item => {
    const [id, ...parts] = item.split(':');
    const definition = loaded.contract.artifacts.find(a => a.id === id);
    const file = parts.join(':');
    if (!definition || !validArtifactName(file, definition.format)) throw new Error(`Unsupported artifact: ${item}`);
    if (definition.format === 'streamDeckPlugin' && file !== `MixRelay-StreamDeck-${productVersion}.streamDeckPlugin`) {
      throw new Error('Stream Deck artifact filename must match product version');
    }
    return { id, file, ...artifactBytes(directory, { file }) };
  });
  if (new Set(artifacts.map(a => a.id)).size !== artifacts.length
    || new Set(artifacts.map(a => a.file)).size !== artifacts.length) throw new Error('Duplicate candidate artifact');
  for (const definition of loaded.contract.artifacts) {
    if (!artifacts.some(a => a.id === definition.id)) throw new Error(`Missing candidate artifact: ${definition.id}`);
  }
  return { schemaVersion: 1, product: loaded.contract.product, version: productVersion,
    sourceRevision, contractSha256: loaded.sha256, artifacts };
}
function validArtifactName(file, format) {
  return !file.includes(':') && file.endsWith(format === 'arch' ? '.pkg.tar.zst' : `.${format}`);
}
export function validateCandidate(candidate, directory, loaded = loadContract()) {
  revision(candidate.sourceRevision);
  if (candidate.schemaVersion !== 1 || candidate.product !== loaded.contract.product
    || candidate.contractSha256 !== loaded.sha256) throw new Error('Candidate support contract mismatch');
  for (const artifact of candidate.artifacts) {
    const actual = artifactBytes(directory, artifact);
    if (actual.sha256 !== artifact.sha256 || actual.size !== artifact.size) throw new Error('Candidate artifact bytes changed');
  }
  const expected = createCandidate(directory, candidate.artifacts.map(a => `${a.id}:${a.file}`),
    candidate.sourceRevision, candidate.version, loaded);
  if (JSON.stringify(expected.artifacts) !== JSON.stringify(candidate.artifacts)) throw new Error('Candidate artifact bytes changed');
  return candidate;
}

export function evaluateQualification(candidate, reports, loaded = loadContract()) {
  const failures = [];
  const targets = [...loaded.contract.qualificationTargets, ...loaded.contract.artifactChecks];
  const runId = reports[0]?.provenance?.runId;
  const runAttempt = reports[0]?.provenance?.runAttempt;
  const seen = new Set();
  for (const report of reports) {
    if (seen.has(report.targetId)) throw new Error(`Duplicate qualification report: ${report.targetId}`);
    seen.add(report.targetId);
    if (!targets.some(t => t.id === report.targetId)) throw new Error(`Unknown qualification target: ${report.targetId}`);
  }
  for (const target of targets) {
    const report = reports.find(r => r.targetId === target.id);
    if (!report) { failures.push(`${target.id}: missing report${target.execution?.kind === 'unavailable' ? `; ${target.execution.reason}` : ''}`); continue; }
    try {
      const artifact = candidate.artifacts.find(a => a.id === target.artifact);
      if (!/^[1-9]\d*$/.test(runId ?? '') || report.provenance?.runId !== runId
        || !/^[1-9]\d*$/.test(runAttempt ?? '') || report.provenance?.runAttempt !== runAttempt
        || report.provenance?.repository !== '0x5253/mixrelay') throw new Error('Missing or mixed CI qualification provenance');
      if (report.schemaVersion !== 1 || report.sourceRevision !== candidate.sourceRevision
        || report.contractSha256 !== loaded.sha256 || report.artifactSha256 !== artifact?.sha256
        || report.candidateSha256 !== digest(JSON.stringify(candidate))) throw new Error('Wrong candidate, source or artifact');
      if (!target.kind) validateEnvironment(report.environment, target, loaded.contract);
      if (!Number.isFinite(Date.parse(report.startedAt)) || !Number.isFinite(Date.parse(report.endedAt))
        || Date.parse(report.endedAt) < Date.parse(report.startedAt)) throw new Error('Missing execution timestamps');
      if (!Array.isArray(report.scenarios) || new Set(report.scenarios.map(s => s.id)).size !== report.scenarios.length) throw new Error('Invalid scenario results');
      for (const expected of requiredScenarios(loaded.contract, target)) {
        const result = report.scenarios.find(s => s.id === expected.id);
        if (!result) throw new Error(`Missing scenario: ${expected.id}`);
        if (result.status !== 'passed') throw new Error(`${expected.id}: ${result.status ?? 'missing status'}`);
        if (result.scope !== expected.scope
          || !Array.isArray(result.evidence) || result.evidence.length === 0
          || result.evidence.some(e => !/^[a-f0-9]{64}$/.test(e.sha256) || !e.file || basename(e.file) !== e.file)) {
          throw new Error(`Incomplete or wrong-scope evidence: ${expected.id}`);
        }
      }
      if (report.cleanup !== 'passed' || report.status !== 'passed') throw new Error('Qualification or cleanup did not pass');
    } catch (error) { failures.push(`${target.id}: ${error.message}`); }
  }
  return { passed: failures.length === 0, failures };
}

export function readReports(directory) {
  return readdirSync(directory, { recursive: true }).filter(file => file.endsWith('.qualification.json'))
    .map(file => {
      const path = resolve(directory, file);
      const report = JSON.parse(readFileSync(path));
      for (const scenario of report.scenarios ?? []) for (const item of scenario.evidence ?? []) {
        if (!item.file || /[\\\0]/.test(item.file) || basename(item.file) !== item.file || digest(readFileSync(resolve(dirname(path), item.file))) !== item.sha256) {
          throw new Error(`Missing or changed raw evidence: ${file}`);
        }
      }
      return report;
    });
}
export function vmInventory(directory) {
  const rows = readdirSync(directory, { recursive: true }).filter(file => basename(file) === 'vm-session.json').map(file => {
    const session = JSON.parse(readFileSync(resolve(directory, file)));
    return [session.targetId, session.accelerator, session.cpuModel,
      session.error ?? (session.guestExitCode === 0 ? 'guest suite completed' : `guest exit ${session.guestExitCode ?? 'not observed'}`),
      session.image?.digest];
  });
  if (!rows.length) return '';
  return '\n## VM execution diagnostics\n\n'
    + '| Environment | Accelerator | CPU model | Result | Pinned image checksum |\n'
    + '| --- | --- | --- | --- | --- |\n'
    + rows.map(row => '| ' + row.map(value => String(value ?? 'not observed').replaceAll('|', '/').replaceAll('\n', ' ')).join(' | ') + ' |').join('\n') + '\n';
}
export function qualificationInventory(candidate, reports, loaded = loadContract()) {
  const result = evaluateQualification(candidate, reports, loaded);
  const cell = value => String(value ?? 'not observed').replaceAll('|', '/').replaceAll('\n', ' ').trim();
  const summary = `# Candidate qualification\n\nSource: ${candidate.sourceRevision}\n\nContract: ${loaded.sha256}\n\n`
    + '| Environment | Artifact SHA-256 | Result |\n| --- | --- | --- |\n'
    + [...loaded.contract.qualificationTargets, ...loaded.contract.artifactChecks].map(t => {
      const failure = result.failures.find(f => f.startsWith(`${t.id}:`));
      return `| ${t.id} | ${candidate.artifacts.find(a => a.id === t.artifact)?.sha256 ?? 'missing'} | ${failure ? failure.replaceAll('|', '/') : 'passed'} |`;
    }).join('\n') + '\n';
  const environments = '\n## Observed installed environments\n\n'
    + '| Environment | OS version | CPU | Kernel | PipeWire | WirePlumber | glibc | Run |\n'
    + '| --- | --- | --- | --- | --- | --- | --- | --- |\n'
    + loaded.contract.qualificationTargets.map(target => {
      const report = reports.find(r => r.targetId === target.id), observed = report?.environment;
      const release = observed?.os === 'linux' && typeof observed.osRelease === 'string' ? parseOsRelease(observed.osRelease) : {};
      const osVersion = release.PRETTY_NAME ? `${release.PRETTY_NAME} (${release.DEBIAN_VERSION_FULL ?? observed.version})` : observed?.version;
      const dependencies = target.os === 'linux'
        ? [observed?.dependencies?.pipewire, observed?.dependencies?.wireplumber, observed?.dependencies?.glibc]
        : ['not applicable', 'not applicable', 'not applicable'];
      return '| ' + [target.id, osVersion, observed?.arch, observed?.kernel,
        ...dependencies,
        report?.provenance?.runId ? `https://github.com/0x5253/mixrelay/actions/runs/${report.provenance.runId}` : undefined].map(cell).join(' | ') + ' |';
    }).join('\n') + '\n';
  const errors = reports.flatMap(report => {
    const failures = (report.scenarios ?? []).filter(s => s.status !== 'passed');
    return [report.error && `${report.targetId}: ${report.error}`,
      report.cleanupError && `${report.targetId} cleanup: ${report.cleanupError}`,
      ...failures.map(s => `${report.targetId} / ${s.id} (${s.status}): ${s.error ?? 'No diagnostic recorded'}`)].filter(Boolean);
  });
  return summary + environments + '\n## Scenario results\n\n'
    + '| Environment | ' + loaded.contract.scenarios.map(s => s.id).join(' | ') + ' |\n'
    + '| --- | ' + loaded.contract.scenarios.map(() => '---').join(' | ') + ' |\n'
    + loaded.contract.qualificationTargets.map(target => {
      const report = reports.find(r => r.targetId === target.id);
      let identityMatches = false;
      if (report) try {
        validateEnvironment(report.environment, target, loaded.contract);
        identityMatches = report.candidateSha256 === digest(JSON.stringify(candidate))
          && report.artifactSha256 === candidate.artifacts.find(a => a.id === target.artifact)?.sha256
          && report.sourceRevision === candidate.sourceRevision && report.contractSha256 === loaded.sha256;
      } catch {}
      const cells = loaded.contract.scenarios.map(scenario => {
        if (!requiredScenarios(loaded.contract, target).some(s => s.id === scenario.id)) return 'not applicable';
        if (!report) return 'not run';
        if (!identityMatches) return 'invalid report';
        const observed = report.scenarios?.find(s => s.id === scenario.id);
        if (!observed) return 'not run';
        if (observed.scope !== scenario.scope) return 'invalid scope';
        return ['passed', 'failed', 'blocked', 'skipped'].includes(observed.status) ? observed.status : 'invalid result';
      });
      return `| ${target.id} | ${cells.join(' | ')} |`;
    }).join('\n') + '\n'
    + (errors.length ? '\n## Failure and blocking diagnostics\n\n'
      + 'Full diagnostics remain in the per-target qualification JSON and raw Actions evidence.\n\n'
      + errors.map(error => `- ${cell(error).slice(0, 700)}${cell(error).length > 700 ? '…' : ''}`).join('\n') + '\n' : '');
}
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    const command = process.argv[2];
    const { values } = parseArgs({ args: process.argv.slice(3), options: {
      directory: { type: 'string' }, candidate: { type: 'string' }, reports: { type: 'string' },
      artifact: { type: 'string', multiple: true, default: [] }, 'source-revision': { type: 'string' },
      version: { type: 'string' }, output: { type: 'string' },
    } });
    const standaloneInventory = command === 'inventory' && !values.candidate && !values.directory && !values.reports;
    if (!standaloneInventory && !values.directory) throw new Error('--directory is required');
    if (standaloneInventory) {
      const output = qualificationInventory({ sourceRevision: 'no candidate selected', artifacts: [] }, []);
      if (values.output) writeFileSync(values.output, output, { flag: 'wx' }); else console.log(output);
    } else if (command === 'candidate') {
      const candidate = createCandidate(values.directory, values.artifact, values['source-revision'], values.version);
      writeFileSync(resolve(values.directory, 'mix-relay.candidate.json'), JSON.stringify(candidate, null, 2) + '\n', { flag: 'wx' });
    } else {
      if (!values.candidate || !values.reports) throw new Error('--candidate and --reports are required');
      const candidate = validateCandidate(JSON.parse(readFileSync(values.candidate)), values.directory);
      const reports = readReports(values.reports);
      if (command === 'inventory') {
        const text = qualificationInventory(candidate, reports) + vmInventory(values.reports);
        if (values.output) writeFileSync(values.output, text, { flag: 'wx' }); else console.log(text);
      } else if (command === 'gate') {
        const result = evaluateQualification(candidate, reports);
        if (!result.passed) throw new Error(result.failures.join('\n'));
        console.log('All required environments qualified the exact candidate artifacts');
      } else throw new Error('Expected candidate, inventory or gate');
    }
  } catch (error) { console.error(error.message); process.exitCode = 1; }
}
