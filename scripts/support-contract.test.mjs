import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';
import { loadContract, validateContract, checkGenerated, releaseAssets, releaseInstallers } from './support-contract.mjs';
import { ciMatrices, requiredScenarios } from './core/support-targets.mjs';

test('support registry drives native multi-architecture CI and checked-in generated consumers', () => {
  const { contract } = loadContract();
  checkGenerated(contract);
  const matrices = ciMatrices(contract);
  assert.equal(matrices.linux.include.length, 10);
  assert.deepEqual(matrices.linuxBuild.include.map(t => [t.bundles, t.rpm, t.archPackage]),
    [['deb,rpm', 'linux-x64-rpm', 'linux-x64-arch'], ['deb,rpm', 'linux-arm64-rpm', 'linux-arm64-arch']]);
  assert.deepEqual(matrices.archBuild.include.map(t => [t.id, t.distro, t.pkgarch, t.runner]), [
    ['linux-x64-arch', 'arch', 'x86_64', 'ubuntu-24.04'],
    ['linux-arm64-arch', 'archarm', 'aarch64', 'ubuntu-24.04-arm']]);
  assert.deepEqual(new Set(matrices.macos.include.map(t => t.arch)), new Set(['x64', 'arm64']));
  assert.deepEqual(matrices.macosPullRequest.include.map(t => t.id).sort(), ['macos-15-x64', 'macos-27-arm64']);
  assert.equal(matrices.linux.include.find(t => t.id === 'debian-arm64').runner, 'ubuntu-24.04-arm');
  // Arch Linux publishes no aarch64 image; Arch Linux ARM is its own distribution and rootfs.
  assert.ok(!matrices.linux.include.some(t => t.id === 'arch-arm64'));
  assert.equal(matrices.linux.include.find(t => t.id === 'archarm-arm64').runner, 'ubuntu-24.04-arm');
  assert.ok(requiredScenarios(contract, { os: 'macos' }).some(s => s.id === 'AUDIO-11'));
  assert.ok(!requiredScenarios(contract, { os: 'linux' }).some(s => s.id === 'AUDIO-11'));
});
test('Stream Deck is a required archive check, separate from installer environments and the update catalog', () => {
  const { contract } = loadContract();
  const check = contract.artifactChecks[0];
  assert.deepEqual(requiredScenarios(contract, check).map(s => s.id), ['STREAMDECK-01']);
  assert.ok(!requiredScenarios(contract, contract.qualificationTargets[0]).some(s => s.id === 'STREAMDECK-01'));
  assert.equal(releaseInstallers({ artifacts: contract.artifacts }, contract).length, 7);
  for (const change of [c => { c.artifactChecks = []; }, c => { c.artifactChecks[0].artifact = 'macos-universal-pkg'; },
    c => { c.artifactChecks[0].id = c.qualificationTargets[0].id; },
    c => { c.scenarios.find(s => s.id === 'STREAMDECK-01').required = false; },
    c => { c.scenarios.find(s => s.id === 'STREAMDECK-01').artifact = 'missing'; }]) {
    const invalid = structuredClone(contract); change(invalid);
    assert.throws(() => validateContract(invalid));
  }
});
test('duplicate environments, unknown artifacts and floating distro images fail contract validation', () => {
  for (const mutate of [c => c.qualificationTargets.push(c.qualificationTargets[0]),
    c => { c.qualificationTargets[0].artifact = 'missing'; },
    c => { c.codeChecks.linuxDistributions[0].image = 'debian:latest'; },
    c => { c.codeChecks.linuxDistributions.find(d => d.rootfs).rootfs.digest = 'changed'; },
    c => { c.codeChecks.linuxDistributions.find(d => d.rootfs).rootfs.url = 'http://example.test/rootfs.tar'; },
    c => { c.codeChecks.linuxDistributions.find(d => d.rootfs).image = 'archlinux:base'; },
    c => { c.codeChecks.linuxDistributions.find(d => d.rootfs).architectures.push('x64'); },
    c => { c.minimums.linux.glibc = "2.39'; false; '"; },
    c => { c.qualificationTargets[0].version = '15.0'; },
    c => { c.macosArchitectures['14'] = ['arm64', 'x64']; },
    c => { c.qualificationTargets[0].version = '14'; },
    c => { c.codeChecks.macos[0].version = '14'; },
    c => { for (const t of c.codeChecks.macos) if (t.arch === 'x64') delete t.pullRequest; },
    c => { c.codeChecks.macos[0].pullRequest = 'yes'; },
    c => { c.qualificationTargets[0].execution = { kind: 'self-hosted' }; },
    c => { c.qualificationImages['debian-x64'].digest = 'changed'; },
    c => { c.qualificationTargets.find(t => t.id === 'debian-13-arm64').execution.runner = 'ubuntu-24.04'; },
    c => { c.qualificationTargets.find(t => t.id === 'macos-27-arm64').arch = 'x64'; },
    c => { c.qualificationTargets[0].arch = 'universal'; }]) {
    const contract = structuredClone(loadContract().contract); mutate(contract);
    assert.throws(() => validateContract(contract));
  }
});

test('MIDI requires the declared ALSA runtime library in both Linux package formats', () => {
  for (const format of ['deb', 'arch']) {
    const contract = structuredClone(loadContract().contract);
    contract.runtimeLibraries.linux.midi[format] = 'missing-midi-runtime';
    assert.throws(() => checkGenerated(contract), /dependency differs|MIDI library differs/);
  }
});

test('DEB MIDI dependencies use Debian names and Arch retains its own ALSA package', () => {
  const linux = JSON.parse(readFileSync(new URL('../apps/desktop-shell/src-tauri/tauri.linux.conf.json', import.meta.url))).bundle.linux;
  assert.ok(linux.deb.depends.includes('libasound2t64'));
  assert.ok(!linux.deb.depends.includes('alsa-lib'), 'Arch/RPM package names cannot be installed by APT');
  const arch = readFileSync(new URL('../packaging/linux/arch/PKGBUILD.in', import.meta.url), 'utf8');
  assert.match(arch, /depends=\([\s\S]*?'alsa-lib'/);
});

test('release assets are named once per artifact with the version and the format suffix', () => {
  const { contract } = loadContract();
  assert.deepEqual(releaseAssets(contract, '1.2.3').map(asset => asset.file), [
    'MixRelay-Installer-1.2.3.pkg', 'MixRelay_1.2.3_amd64.deb', 'MixRelay_1.2.3_arm64.deb',
    'mix-relay-1.2.3-1-x86_64.pkg.tar.zst', 'mix-relay-1.2.3-1-aarch64.pkg.tar.zst', 'MixRelay-1.2.3-1.x86_64.rpm', 'MixRelay-1.2.3-1.aarch64.rpm',
    'MixRelay-StreamDeck-1.2.3.streamDeckPlugin',
  ]);
  for (const version of ['1.2', 'v1.2.3', '1.2.3-beta', '01.2.3']) assert.throws(() => releaseAssets(contract, version));
  for (const asset of ['MixRelay-1.2.3.rpm', 'MixRelay-{version}.deb', 'MixRelay-{version}-{version}.rpm',
    '../MixRelay-{version}.rpm', 'MixRelay {version}.rpm']) {
    const invalid = structuredClone(contract);
    invalid.artifacts.find(artifact => artifact.id === 'linux-x64-rpm').asset = asset;
    assert.throws(() => validateContract(invalid), /Invalid release asset name/, asset);
  }
  const duplicate = structuredClone(contract);
  duplicate.artifacts.find(artifact => artifact.id === 'linux-arm64-rpm').asset = 'MixRelay-{version}-1.x86_64.rpm';
  assert.throws(() => validateContract(duplicate), /Duplicate release asset names/);
});
