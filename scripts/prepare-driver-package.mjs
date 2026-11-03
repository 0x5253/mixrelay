import { spawnSync } from 'node:child_process';
import * as fs from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { parseArgs, resolveTarget, REPO_ROOT } from './prepare-desktop-engine.mjs';

/** The published macOS Driver service carries exactly the two supported slices. */
export const DRIVER_ARCHITECTURES = ['arm64', 'x86_64'];

function assertUniversalDriver(run, rootDir, env, path) {
  const result = run('xcrun', ['lipo', '-archs', path], { cwd: rootDir, env, encoding: 'utf8', stdio: ['ignore', 'pipe', 'inherit'] });
  if (result.error || result.status !== 0 || result.signal) throw new Error('Driver build failed: cannot read merged Driver service architectures');
  const found = String(result.stdout ?? '').trim().split(/\s+/).filter(Boolean).sort();
  if (found.join(' ') !== DRIVER_ARCHITECTURES.join(' ')) {
    throw new Error(`Driver service must contain exactly ${DRIVER_ARCHITECTURES.join(' and ')}; found: ${found.join(' ') || 'none'}`);
  }
}

/**
 * Build the actual Driver service and prepare the matching package receipt.
 * `--prebuilt` uses slices from an earlier combined cargo build and only merges and signs.
 */
export function prepareDriverPackage({ args = [], env = process.env, rootDir = REPO_ROOT, fsApi = fs, run = spawnSync } = {}) {
  const prebuilt = args.includes('--prebuilt');
  const parsed = parseArgs(args.filter(arg => arg !== '--prebuilt'));
  if (parsed.help) return { status: 'help' };
  const { target, platform } = resolveTarget({ requestedTarget: parsed.target, env, rootDir, run });
  if (platform === 'linux' && !/^(?:aarch64|x86_64)-.*linux/.test(target)) {
    throw new Error('Driver Linux packages support only arm64 and x86_64');
  }
  const config = JSON.parse(fsApi.readFileSync(join(rootDir, 'apps/desktop/src-tauri/tauri.conf.json'), 'utf8'));
  const cargo = fsApi.readFileSync(join(rootDir, 'Cargo.toml'), 'utf8');
  const version = cargo.match(/\[workspace\.package\]([\s\S]*?)(?=\n\[|$)/)?.[1].match(/^version\s*=\s*"([^"]+)"/m)?.[1];
  if (!version || version !== config.version) throw new Error('Driver service and desktop package versions must match');
  const targetRoot = env.CARGO_TARGET_DIR ? resolve(rootDir, env.CARGO_TARGET_DIR) : join(rootDir, 'target');
  const profile = parsed.profile === 'release' ? 'release' : 'debug';
  const binary = 'mixrelay-driver-service';
  const slices = platform === 'darwin' ? ['aarch64-apple-darwin', 'x86_64-apple-darwin'] : [target];
  const checked = (command, commandArgs) => {
    const result = run(command, commandArgs, { cwd: rootDir, env, stdio: 'inherit' });
    if (result.error || result.status !== 0 || result.signal) throw new Error(`Driver build failed: ${command}`);
  };
  for (const slice of slices) {
    if (prebuilt) {
      const sliceOutput = join(targetRoot, slice, profile, binary);
      if (!fsApi.existsSync(sliceOutput)) throw new Error(`Prebuilt Driver service is missing: ${sliceOutput}`);
      continue;
    }
    const cargoArgs = ['build', '--locked', '--package', binary, '--target', slice];
    if (profile === 'release') cargoArgs.push('--release');
    checked('cargo', cargoArgs);
  }
  let output = join(targetRoot, target, profile, binary);
  if (platform === 'darwin') {
    const identity = env.MIXRELAY_CODESIGN_IDENTITY || env.APPLE_SIGNING_IDENTITY;
    if (profile === 'release' && !identity?.startsWith('Developer ID Application: ')) {
      throw new Error('Driver release service requires a Developer ID Application identity');
    }
    output = join(targetRoot, 'universal-apple-darwin', profile, binary);
    fsApi.mkdirSync(dirname(output), { recursive: true });
    const staging = fsApi.mkdtempSync(join(dirname(output), '.driver-service-'));
    try {
      const staged = join(staging, binary);
      checked('xcrun', ['lipo', '-create', ...slices.map(slice => join(targetRoot, slice, profile, binary)), '-output', staged]);
      assertUniversalDriver(run, rootDir, env, staged);
      fsApi.chmodSync(staged, 0o755);
      const signing = ['--force', '--sign', profile === 'release' ? identity : '-'];
      if (profile === 'release') signing.push('--options', 'runtime', '--timestamp', '--identifier', binary);
      checked('codesign', [...signing, staged]);
      checked('codesign', ['--verify', '--strict', '--all-architectures', staged]);
      fsApi.renameSync(staged, output);
    } finally {
      fsApi.rmSync(staging, { recursive: true, force: true });
    }
  } else {
    const sidecar = join(rootDir, 'apps/desktop/src-tauri/binaries', `${binary}-${target}`);
    fsApi.mkdirSync(dirname(sidecar), { recursive: true });
    fsApi.copyFileSync(output, sidecar);
    // RPM preserves sidecar permissions; a restrictive build umask must not
    // make the root-owned installed service inaccessible to ordinary users.
    fsApi.chmodSync(sidecar, 0o755);
  }
  const receipt = join(rootDir, 'target/driver-package/version.json');
  fsApi.mkdirSync(dirname(receipt), { recursive: true });
  fsApi.writeFileSync(receipt, `${JSON.stringify({ version })}\n`);
  return { status: 'prepared', output, receipt, version };
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  try {
    const result = prepareDriverPackage({ args: process.argv.slice(2) });
    console.log(result.status === 'help' ? 'Usage: node scripts/prepare-driver-package.mjs [--profile dev|release] [--target TARGET] [--prebuilt]' : `Prepared Driver service ${result.output}`);
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
