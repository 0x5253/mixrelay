import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createRequire } from 'node:module';
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test, { before, after } from 'node:test';
import { buildStreamDeckArtifact, qualifyStreamDeckArtifact, validatePackedStreamDeck, validateStreamDeckManifest } from './streamdeck-artifact.mjs';
import { qualificationFixture } from './e2e/qualification.fixture.mjs';
import { createCandidate, evaluateQualification, readReports } from './qualification.mjs';
import { digest } from './support-contract.mjs';
import { productVersion } from './product-version.mjs';

const version = productVersion();
const plugin = fileURLToPath(new URL('../apps/streamdeck-plugin/', import.meta.url));
const require = createRequire(join(plugin, 'package.json'));
const cli = join(dirname(require.resolve('@elgato/cli/package.json')), 'bin/streamdeck.mjs');
let temporary, archive;
before(() => {
  temporary = mkdtempSync(join(tmpdir(), 'mixrelay-streamdeck-tests-'));
  execFileSync(process.execPath, ['build.mjs', '--output', join(temporary, 'built/com.rawsector.mixrelay.sdPlugin')], { cwd: plugin });
  execFileSync(process.execPath, [cli, 'pack', join(temporary, 'built/com.rawsector.mixrelay.sdPlugin'), '--output', temporary]);
  archive = join(temporary, 'com.rawsector.mixrelay.streamDeckPlugin');
});
after(() => rmSync(temporary, { recursive: true, force: true }));

test('the packed distribution passes the actual CLI and uses the product identity, DRM and runtime', () => {
  const { manifest, output } = validatePackedStreamDeck(archive, version);
  assert.equal(output, '✔ Validation successful');
  for (const change of [m => { m.SDKVersion = 2; }, m => { m.Software.MinimumVersion = '6.6'; },
    m => { m.OS[0].MinimumVersion = '14.9'; }, m => { m.OS[0].MinimumVersion = '16.0'; },
    m => { m.Nodejs.Version = '20'; }, m => { m.UUID = 'io.mixrelay.streamdeck'; },
    m => { m.Version = `${version}.7`; }, m => { m.Actions[0].UUID = 'com.rawsector.mixrelay.node'; }]) {
    const invalid = structuredClone(manifest); change(invalid);
    assert.throws(() => validateStreamDeckManifest(invalid, version));
  }
});

test('an isolated build refuses to clear a directory outside the plugin bundle', () => {
  assert.throws(() => execFileSync(process.execPath, ['build.mjs', '--output', temporary],
    { cwd: plugin, stdio: 'pipe' }), /--output must name com\.rawsector\.mixrelay\.sdPlugin/);
  assert.ok(readFileSync(archive).length > 0);
});

test('archive paths, duplicate entries and CLI warnings cannot pass qualification', () => {
  for (const listing of ['../escape', 'com.rawsector.mixrelay.sdPlugin/../../escape',
    '/absolute', 'com.rawsector.mixrelay.sdPlugin/bin/[wildcard]',
    'com.rawsector.mixrelay.sdPlugin/manifest.json\ncom.rawsector.mixrelay.sdPlugin/manifest.json']) {
    assert.throws(() => validatePackedStreamDeck(archive, version, () => listing), /Unsafe|Duplicate/);
  }
  const run = (program, args, options) => program === process.execPath
    ? '✔ Validation successful\n1 warning'
    : execFileSync(program, args, { encoding: 'utf8', ...options });
  assert.throws(() => validatePackedStreamDeck(archive, version, run), /warning/);
});

test('release build names and hashes the exact package and refuses to overwrite it or use a different version', () => {
  const output = join(temporary, 'release');
  const calls = [];
  const run = (program, args, options) => {
    calls.push([program, args]);
    if (program !== 'pnpm') return execFileSync(program, args, { encoding: 'utf8', ...options });
    cpSync(archive, join(plugin, 'dist/com.rawsector.mixrelay.streamDeckPlugin'));
  };
  const result = buildStreamDeckArtifact(output, version, run);
  assert.equal(result.file, `MixRelay-StreamDeck-${version}.streamDeckPlugin`);
  assert.equal(result.sha256, digest(readFileSync(archive)));
  assert.equal(result.size, readFileSync(archive).length);
  assert.deepEqual(calls[0], ['pnpm', ['--filter', '@mixrelay/streamdeck-plugin', 'run', 'pack']]);
  assert.throws(() => buildStreamDeckArtifact(output, version, run), /EEXIST/);
  assert.throws(() => buildStreamDeckArtifact(output, '2.0.0', run), /product version/);
});

test('the gate requires a hash-bound archive report and rejects changed package bytes or missing evidence', t => {
  const f = qualificationFixture(t);
  const unpacked = join(temporary, 'com.rawsector.mixrelay.sdPlugin');
  cpSync(join(temporary, 'built/com.rawsector.mixrelay.sdPlugin'), unpacked, { recursive: true });
  const manifest = JSON.parse(readFileSync(join(unpacked, 'manifest.json')));
  manifest.Version = '1.2.3.0';
  writeFileSync(join(unpacked, 'manifest.json'), JSON.stringify(manifest));
  const packed = join(f.directory, 'MixRelay-StreamDeck-1.2.3.streamDeckPlugin');
  rmSync(packed);
  execFileSync('zip', ['-qr', packed, 'com.rawsector.mixrelay.sdPlugin'], { cwd: temporary });
  const candidate = createCandidate(f.directory, f.candidate.artifacts.map(a => `${a.id}:${a.file}`), f.candidate.sourceRevision, '1.2.3');
  writeFileSync(f.candidatePath, JSON.stringify(candidate));
  const evidence = join(f.directory, 'archive-evidence');
  const report = qualifyStreamDeckArtifact(f.directory, evidence, { GITHUB_REPOSITORY: '0x5253/mixrelay', GITHUB_RUN_ID: '42', GITHUB_RUN_ATTEMPT: '1' });
  assert.equal(report.artifactSha256, digest(readFileSync(packed)));
  assert.equal(readReports(evidence).length, 1);
  const reports = f.reports.filter(r => r.targetId !== 'streamdeck-plugin').map(r => ({ ...r, candidateSha256: digest(JSON.stringify(candidate)) }));
  assert.equal(evaluateQualification(candidate, [...reports, report]).passed, true);
  assert.match(evaluateQualification(candidate, reports).failures.join('\n'), /streamdeck-plugin: missing report/);
  writeFileSync(join(evidence, 'streamdeck-validation.json'), 'tampered');
  assert.throws(() => readReports(evidence), /changed raw evidence/);
  writeFileSync(packed, 'tampered package');
  assert.throws(() => qualifyStreamDeckArtifact(f.directory, join(f.directory, 'other-evidence')), /bytes changed/);
});
