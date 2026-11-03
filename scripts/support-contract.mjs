import { ciMatrices } from './core/support-targets.mjs';
import { createHash } from 'node:crypto';
import { readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { syncProductMetadata } from './product-metadata.mjs';
import { validateProductMetadata } from './core/product-metadata-policy.mjs';

export const contractPath = fileURLToPath(new URL('../support/environments.json', import.meta.url));
export const digest = bytes => createHash('sha256').update(bytes).digest('hex');
export function loadContract(path = contractPath) {
  const bytes = readFileSync(path);
  const contract = JSON.parse(bytes);
  validateContract(contract);
  return { contract, sha256: digest(bytes) };
}

function unique(items, label) {
  const ids = items.map(item => item.id);
  if (ids.some(id => !/^[A-Za-z0-9][A-Za-z0-9.-]*$/.test(id)) || new Set(ids).size !== ids.length) {
    throw new Error(`Invalid or duplicate ${label} IDs`);
  }
}

export function validateContract(value) {
  if (value.schemaVersion !== 1 || value.product !== 'mix-relay') throw new Error('Unsupported support contract');
  validateProductMetadata(value.metadata);
  if (!/^\d+\.\d+$/.test(value.minimums?.macos ?? '')
    || !['pipewire', 'wireplumber', 'glibc'].every(name => /^\d+\.\d+(?:\.\d+)?$/.test(value.minimums?.linux?.[name] ?? ''))) {
    throw new Error('Invalid deployment or dependency minimum');
  }
  if (Object.keys(value.macosArchitectures ?? {}).some(major => Number(major) < Number(value.minimums.macos.split('.')[0]))) {
    throw new Error('macOS architecture support is below the deployment minimum');
  }
  for (const [items, label] of [[value.artifacts, 'artifact'], [value.qualificationTargets, 'environment'],
    [value.scenarios, 'scenario'], [value.codeChecks?.macos, 'macOS CI'],
    [value.codeChecks?.linuxDistributions, 'Linux CI']]) {
    if (!Array.isArray(items) || items.length === 0) throw new Error(`Missing ${label} definitions`);
    unique(items, label);
  }
  if (!Array.isArray(value.artifactChecks)) throw new Error('Missing artifact checks');
  unique(value.artifactChecks, 'artifact check');
  unique([...value.qualificationTargets, ...value.artifactChecks], 'qualification');
  for (const check of value.artifactChecks) {
    if (check.kind !== 'streamdeck' || !value.artifacts.some(a => a.id === check.artifact && a.format === 'streamDeckPlugin')) {
      throw new Error(`Invalid artifact check: ${check.id}`);
    }
  }
  for (const artifact of value.artifacts) {
    if (!(artifact.os === 'macos' && artifact.arch === 'universal' && artifact.format === 'pkg')
      && !(artifact.os === 'macos' && artifact.arch === 'universal' && artifact.format === 'streamDeckPlugin')
      && !(artifact.os === 'linux' && ['x64', 'arm64'].includes(artifact.arch) && ['deb', 'arch'].includes(artifact.format))) throw new Error(`Invalid artifact definition: ${artifact.id}`);
    if (!(artifact.format === 'streamDeckPlugin' ? value.artifactChecks : value.qualificationTargets)
      .some(t => t.artifact === artifact.id)) throw new Error(`Artifact has no qualification target: ${artifact.id}`);
  }
  for (const target of value.qualificationTargets) {
    const artifact = value.artifacts.find(item => item.id === target.artifact);
    if (!artifact || artifact.os !== target.os || ![target.arch, 'universal'].includes(artifact.arch)) {
      throw new Error(`Invalid artifact mapping: ${target.id}`);
    }
    if (!['major', 'minor', 'rolling'].includes(target.versionMatch)
      || !['x64', 'arm64'].includes(target.arch)
      || !['github-hosted', 'linux-vm', 'unavailable'].includes(target.execution?.kind)) {
      throw new Error(`Incomplete qualification environment: ${target.id}`);
    }
    if (target.versionMatch !== 'rolling' && !new RegExp(target.versionMatch === 'major' ? '^\\d+$' : '^\\d+\\.\\d+$').test(target.version)) throw new Error(`Invalid version rule: ${target.id}`);
    if (target.os === 'macos' && !value.macosArchitectures?.[target.version.split('.')[0]]?.includes(target.arch)) throw new Error(`Unsupported macOS/CPU combination: ${target.id}`);
    const execution = target.execution;
    if (execution.kind === 'unavailable') {
      if (!execution.reason) throw new Error(`Missing coverage gap reason: ${target.id}`);
    } else if (target.os === 'macos') {
      if (execution.kind !== 'github-hosted' || !value.codeChecks.macos.some(t => t.runner === execution.runner
        && t.arch === target.arch && t.version === target.version)) throw new Error(`Invalid hosted macOS target: ${target.id}`);
    } else if (execution.kind !== 'linux-vm' || execution.runner !== (target.arch === 'arm64' ? 'ubuntu-24.04-arm' : 'ubuntu-24.04')
      || !value.qualificationImages?.[execution.image]) throw new Error(`Invalid Linux VM target: ${target.id}`);
  }
  for (const image of Object.values(value.qualificationImages ?? {})) {
    if (!/^https:\/\//.test(image.url) || !['qcow2', 'tar.xz'].includes(image.format)
      || !['sha256', 'sha512'].includes(image.algorithm)
      || !new RegExp(`^[a-f0-9]{${image.algorithm === 'sha512' ? 128 : 64}}$`).test(image.digest)) throw new Error('Unpinned qualification VM image');
  }
  for (const target of value.codeChecks.macos) {
    if (!value.macosArchitectures?.[target.version]?.includes(target.arch)) throw new Error(`Unsupported macOS CI combination: ${target.id}`);
    if (target.pullRequest !== undefined && target.pullRequest !== true) throw new Error(`Invalid pull request flag: ${target.id}`);
  }
  const pullRequestArchitectures = new Set(value.codeChecks.macos.filter(t => t.pullRequest).map(t => t.arch));
  if (!pullRequestArchitectures.has('arm64') || !pullRequestArchitectures.has('x64')) {
    throw new Error('Pull request macOS checks must cover arm64 and x64');
  }
  for (const distro of value.codeChecks.linuxDistributions) {
    if (!/^\S+@sha256:[a-f0-9]{64}$/.test(distro.image)) throw new Error(`Unpinned image: ${distro.id}`);
  }
  if (!value.codeChecks.linuxDistributions.some(d => d.id === value.build?.linuxDistribution && d.artifactFormat === 'deb')) throw new Error('Invalid Linux release build baseline');
  for (const scenario of value.scenarios) {
    if (!['artifact', 'installed-api', 'installed-ui', 'native-audio'].includes(scenario.scope)
      || typeof scenario.required !== 'boolean') throw new Error(`Invalid scenario: ${scenario.id}`);
    if (scenario.artifact && (scenario.scope !== 'artifact'
      || !value.artifactChecks.some(check => check.artifact === scenario.artifact))) throw new Error(`Invalid artifact scenario: ${scenario.id}`);
  }
  for (const check of value.artifactChecks) {
    if (!value.scenarios.some(s => s.required && s.artifact === check.artifact)) throw new Error(`Artifact check has no required scenario: ${check.id}`);
  }
}

export function qualificationMatrix(contract) {
  return { include: contract.qualificationTargets.filter(t => t.execution.kind !== 'unavailable')
    .map(target => ({ ...target, runner: target.execution.runner })) };
}

export function composeDocument(contract) {
  const services = contract.codeChecks.linuxDistributions.map(distro =>
    `  ${distro.id}:\n    <<: *runtime\n${distro.id === 'arch' ? '    platform: linux/amd64\n' : ''}`
      + `    build:\n      <<: *build\n      args:\n        DISTRO_IMAGE: ${distro.image}\n`).join('');
  return '# Generated by pnpm support:sync. Edit support/environments.json.\n'
    + 'name: mixrelay-lab\nx-build: &build\n  context: ../..\n  dockerfile: infra/lab/Dockerfile\n'
    + 'x-runtime: &runtime\n  init: true\n  cap_drop: [ALL]\n  security_opt: [no-new-privileges:true]\nservices:\n' + services;
}

export const releaseInstallers = (candidate, contract) => candidate.artifacts.filter(artifact =>
  contract.artifacts.find(definition => definition.id === artifact.id)?.format !== 'streamDeckPlugin');

export function checkGenerated(contract, root = fileURLToPath(new URL('..', import.meta.url)), write = false) {
  syncProductMetadata(contract.metadata, root, write);
  for (const [relative, expected] of [
    ['infra/lab/compose.yaml', composeDocument(contract)],
  ]) {
    const path = resolve(root, relative);
    if (write) writeFileSync(path, expected);
    else if (readFileSync(path, 'utf8') !== expected) throw new Error(`Support definition drift: ${relative}; run pnpm support:sync`);
  }
  const tauri = JSON.parse(readFileSync(resolve(root, 'apps/desktop-shell/src-tauri/tauri.conf.json')));
  if (tauri.bundle.macOS.minimumSystemVersion !== contract.minimums.macos) throw new Error('macOS deployment minimum differs from support contract');
  const linux = JSON.parse(readFileSync(resolve(root, 'apps/desktop-shell/src-tauri/tauri.linux.conf.json'))).bundle.linux;
  if (linux.deb.depends.includes(contract.runtimeLibraries.linux.midi.arch)) throw new Error('DEB MIDI dependency uses an Arch package name');
  for (const dependency of [`libc6 (>= ${contract.minimums.linux.glibc})`,
    `pipewire (>= ${contract.minimums.linux.pipewire})`, `wireplumber (>= ${contract.minimums.linux.wireplumber})`, contract.runtimeLibraries.linux.midi.deb]) {
    if (!linux.deb.depends.includes(dependency)) throw new Error(`Linux dependency differs from support contract: ${dependency}`);
  }
  if (!readFileSync(resolve(root, 'packaging/linux/arch/PKGBUILD.in'), 'utf8').includes(`'${contract.runtimeLibraries.linux.midi.arch}'`)) throw new Error('Arch MIDI library differs from support contract');
  if (!readFileSync(resolve(root, 'native/linux/pipewire/build.rs'), 'utf8')
    .includes(`.atleast_version("${contract.minimums.linux.pipewire}")`)) throw new Error('PipeWire build floor differs from support contract');
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    const { contract } = loadContract();
    const command = process.argv[2];
    if (command === 'matrix') {
      const matrices = ciMatrices(contract);
      for (const name of ['macos', 'macosPullRequest', 'linux', 'linuxBuild']) console.log(`${name}=${JSON.stringify(matrices[name])}`);
    } else if (command === 'targets') console.log(JSON.stringify({ include: contract.qualificationTargets }));
    else if (command === 'sync' || command === 'check') {
      checkGenerated(contract, undefined, command === 'sync');
      console.log(`Support contract ${command} passed`);
    } else throw new Error('Usage: node scripts/support-contract.mjs check|sync|matrix|targets');
  } catch (error) { console.error(error.message); process.exitCode = 1; }
}
