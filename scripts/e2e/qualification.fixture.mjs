// Synthetic gate fixtures. These files are never installed or used as release evidence.
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { digest, loadContract } from '../support-contract.mjs';
import { requiredScenarios } from '../core/support-targets.mjs';
import { createCandidate } from '../qualification.mjs';

export function qualificationFixture(t) {
  const parent = fileURLToPath(new URL('../../target/test-artifacts/', import.meta.url));
  mkdirSync(parent, { recursive: true });
  const directory = mkdtempSync(join(parent, 'qualification-unit-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const loaded = loadContract();
  const selections = loaded.contract.artifacts.map(artifact => {
    const names = { 'macos-universal-pkg': 'MixRelay Install.pkg', 'linux-x64-deb': 'MixRelay-x64.deb',
      'linux-arm64-deb': 'MixRelay-arm64.deb', 'linux-x64-arch': 'MixRelay-x64.pkg.tar.zst',
      'linux-arm64-arch': 'MixRelay-arm64.pkg.tar.zst',
      'linux-x64-rpm': 'MixRelay-x64.rpm', 'linux-arm64-rpm': 'MixRelay-arm64.rpm', 'macos-universal-streamdeck': 'MixRelay-StreamDeck-1.2.3.streamDeckPlugin' };
    const file = names[artifact.id];
    writeFileSync(join(directory, file), `Synthetic ${artifact.id} artifact fixture`);
    return `${artifact.id}:${file}`;
  });
  const candidate = createCandidate(directory, selections, 'a'.repeat(40), '1.2.3', loaded);
  const candidatePath = join(directory, 'mix-relay.candidate.json');
  writeFileSync(candidatePath, JSON.stringify(candidate));
  const reportsPath = join(directory, 'reports');
  mkdirSync(reportsPath);
  const reports = [...loaded.contract.qualificationTargets, ...loaded.contract.artifactChecks].map(target => {
    const dir = join(reportsPath, target.id); mkdirSync(dir);
    const bytes = Buffer.from('Synthetic scenario evidence\n');
    writeFileSync(join(dir, 'unit-evidence.txt'), bytes);
    const report = { schemaVersion: 1, targetId: target.id, sourceRevision: candidate.sourceRevision,
      contractSha256: loaded.sha256, candidateSha256: digest(JSON.stringify(candidate)),
      artifactSha256: candidate.artifacts.find(item => item.id === target.artifact).sha256,
      provenance: { repository: '0x5253/mixrelay', runId: '42', runAttempt: '1' },
      environment: { os: target.os, arch: target.arch, distro: target.distro,
        version: target.version === 'rolling' ? '2026.4' : target.id === 'macos-15-arm64' ? '15.7.1' : target.version,
        kind: 'native-session', kernel: 'synthetic-test-kernel', osRelease: 'Synthetic OS release',
        packageInventorySha256: 'f'.repeat(64), dependencies: { pipewire: '1.2.0', wireplumber: '0.5.0', glibc: '2.39' } },
      status: 'passed', cleanup: 'passed', startedAt: '2026-10-23T00:00:00Z', endedAt: '2026-10-23T00:00:01Z',
      scenarios: requiredScenarios(loaded.contract, target).map(scenario => ({ id: scenario.id, scope: scenario.scope,
        status: 'passed', evidence: [{ file: 'unit-evidence.txt', sha256: digest(bytes) }] })) };
    writeFileSync(join(dir, `${target.id}.qualification.json`), JSON.stringify(report));
    return report;
  });
  const artifacts = candidate.artifacts.filter(artifact => artifact.id !== 'macos-universal-streamdeck').map(artifact => {
    const definition = loaded.contract.artifacts.find(item => item.id === artifact.id);
    return { os: definition.os, arch: definition.arch, file: artifact.file };
  });
  return { directory, candidate, candidatePath, reports, reportsPath, loaded, artifacts,
    qualificationArgs: ['--candidate', candidatePath, '--reports', reportsPath],
    artifactArgs: artifacts.flatMap(artifact => ['--artifact', `${artifact.os}:${artifact.arch}:${artifact.file}`]) };
}
