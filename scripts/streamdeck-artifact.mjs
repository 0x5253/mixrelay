import { MINIMUM_MACOS } from './macos-minimum.mjs';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createRequire } from 'node:module';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { parseArgs, stripVTControlCharacters } from 'node:util';
import { productVersion } from './product-version.mjs';
import { artifactBytes, validateCandidate } from './qualification.mjs';
import { digest, loadContract } from './support-contract.mjs';

const root = fileURLToPath(new URL('..', import.meta.url));
const plugin = join(root, 'apps/streamdeck-plugin');
const require = createRequire(join(plugin, 'package.json'));
const uuid = 'com.rawsector.mixrelay';
export const streamDeckFilename = version => `MixRelay-StreamDeck-${version}.streamDeckPlugin`;

function execute(program, args, options = {}) {
  return execFileSync(program, args, { encoding: 'utf8', timeout: 60000, maxBuffer: 16 * 1024 * 1024, ...options });
}
export function validateStreamDeckManifest(manifest, version) {
  assert.equal(manifest.UUID, uuid, 'Wrong Stream Deck plugin identity');
  assert.equal(manifest.SDKVersion, 3, 'Stream Deck plugin must support DRM');
  assert.equal(manifest.Software.MinimumVersion, '7.1', 'SDK 3.x requires Stream Deck 7.1');
  assert.equal(manifest.Nodejs.Version, '24');
  assert.equal(manifest.Version, `${version}.0`, 'Stream Deck version differs from product');
  assert.equal(manifest.Author, 'rawsector');
  assert.equal(manifest.Name, 'MixRelay');
  assert.equal(manifest.Category, 'MixRelay');
  assert.deepEqual(manifest.OS, [{ Platform: 'mac', MinimumVersion: MINIMUM_MACOS }]);
  assert.deepEqual(manifest.Actions.map(action => action.UUID).sort(),
    ['power', 'mute', 'volume', 'recording'].map(action => `${uuid}.${action}`).sort());
}

export function validatePackedStreamDeck(file, version, run = execute) {
  const temporary = mkdtempSync(join(tmpdir(), 'mixrelay-streamdeck-'));
  try {
    const names = run('unzip', ['-Z1', resolve(file)]).trim().split('\n');
    assert.equal(new Set(names).size, names.length, 'Duplicate plugin archive entry');
    for (const name of names) {
      assert.ok(name.startsWith(`${uuid}.sdPlugin/`) && /^[A-Za-z0-9_./@+-]+$/.test(name)
        && !name.split('/').some(part => part === '.' || part === '..'), 'Unsafe plugin archive entry');
      const path = join(temporary, name);
      if (name.endsWith('/')) mkdirSync(path, { recursive: true });
      else {
        // Write bytes ourselves: archive symlinks must never be followed during extraction.
        const bytes = run('unzip', ['-p', resolve(file), name], { encoding: 'buffer' });
        mkdirSync(dirname(path), { recursive: true });
        writeFileSync(path, bytes, { flag: 'wx' });
      }
    }
    const directory = join(temporary, `${uuid}.sdPlugin`);
    const manifest = JSON.parse(readFileSync(join(directory, 'manifest.json')));
    validateStreamDeckManifest(manifest, version);
    const cli = join(dirname(require.resolve('@elgato/cli/package.json')), 'bin/streamdeck.mjs');
    // CI runners force colour; the recorded evidence keeps only the CLI's text.
    const output = stripVTControlCharacters(run(process.execPath, [cli, 'validate', directory]));
    assert.match(output, /Validation successful/, 'Stream Deck CLI must pass without warnings');
    assert.doesNotMatch(output, /\b(?:warning|error)s?\b/i);
    return { manifest, output: output.trim() };
  } finally {
    rmSync(temporary, { recursive: true, force: true });
  }
}

export function buildStreamDeckArtifact(output, version = productVersion(), run = execute) {
  assert.equal(version, productVersion(), 'Build must use the product version');
  run('pnpm', ['--filter', '@mixrelay/streamdeck-plugin', 'run', 'pack'], { cwd: root, stdio: 'inherit' });
  const source = join(plugin, 'dist', `${uuid}.streamDeckPlugin`);
  const validation = validatePackedStreamDeck(source, version, run);
  mkdirSync(output, { recursive: true });
  const file = streamDeckFilename(version);
  writeFileSync(join(output, file), readFileSync(source), { flag: 'wx' });
  return { file, ...artifactBytes(output, { file }), validation: validation.output };
}

export function qualifyStreamDeckArtifact(directory, output, env = process.env, run = execute) {
  const loaded = loadContract();
  const candidate = validateCandidate(JSON.parse(readFileSync(join(directory, 'mix-relay.candidate.json'))), directory, loaded);
  const target = loaded.contract.artifactChecks.find(check => check.kind === 'streamdeck');
  const artifact = candidate.artifacts.find(item => item.id === target.artifact);
  const startedAt = new Date().toISOString();
  const validation = validatePackedStreamDeck(join(directory, artifact.file), candidate.version, run);
  mkdirSync(output, { recursive: true });
  const evidence = Buffer.from(JSON.stringify({ ...validation, artifact }, null, 2) + '\n');
  writeFileSync(join(output, 'streamdeck-validation.json'), evidence, { flag: 'wx' });
  const report = { schemaVersion: 1, targetId: target.id, sourceRevision: candidate.sourceRevision,
    contractSha256: loaded.sha256, candidateSha256: digest(JSON.stringify(candidate)), artifactSha256: artifact.sha256,
    provenance: { repository: env.GITHUB_REPOSITORY, runId: env.GITHUB_RUN_ID, runAttempt: env.GITHUB_RUN_ATTEMPT },
    startedAt, endedAt: new Date().toISOString(), status: 'passed', cleanup: 'passed',
    scenarios: [{ id: 'STREAMDECK-01', scope: 'artifact', status: 'passed',
      evidence: [{ file: 'streamdeck-validation.json', sha256: digest(evidence) }] }] };
  writeFileSync(join(output, `${target.id}.qualification.json`), JSON.stringify(report, null, 2) + '\n', { flag: 'wx' });
  return report;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    const { values, positionals } = parseArgs({ allowPositionals: true, options: {
      output: { type: 'string' }, directory: { type: 'string' },
    } });
    assert.ok(values.output, '--output is required');
    if (positionals[0] === 'build') console.log(JSON.stringify(buildStreamDeckArtifact(resolve(values.output)), null, 2));
    else if (positionals[0] === 'qualify') {
      assert.ok(values.directory, '--directory is required');
      console.log(JSON.stringify(qualifyStreamDeckArtifact(resolve(values.directory), resolve(values.output)), null, 2));
    } else throw new Error('Usage: streamdeck-artifact.mjs build --output DIRECTORY | qualify --directory CANDIDATE --output EVIDENCE');
  } catch (error) { console.error(error.message); process.exitCode = 1; }
}
