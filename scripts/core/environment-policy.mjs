import { matchesVersion, ciMatrices } from './support-targets.mjs';

export function parseOsRelease(text) {
  return Object.fromEntries(text.split('\n').filter(line => /^[A-Z_]+=/.test(line)).map(line => {
    const index = line.indexOf('=');
    return [line.slice(0, index), line.slice(index + 1).replace(/^"(.*)"$/, '$1')];
  }));
}
export function versionAtLeast(actual, minimum) {
  const parts = value => value.split('.').map(Number);
  const left = parts(actual), right = parts(minimum);
  if (![...left, ...right].every(Number.isSafeInteger)) return false;
  for (let i = 0; i < Math.max(left.length, right.length); i++) {
    const difference = (left[i] ?? 0) - (right[i] ?? 0);
    if (difference !== 0) return difference > 0;
  }
  return true;
}
export function validateEnvironment(observed, target, contract) {
  if (observed.os !== target.os || observed.arch !== target.arch || observed.translated
    || observed.kind !== 'native-session') throw new Error(`Wrong native qualification host: ${target.id}`);
  if (target.distro && observed.distro !== target.distro) throw new Error(`Wrong distribution: ${target.id}`);
  if (!matchesVersion(observed.version, target)) throw new Error(`Wrong OS version: ${target.id}: ${observed.version}`);
  if (observed.os === 'macos' && !versionAtLeast(observed.version, contract.minimums.macos)) throw new Error(`Unsupported macOS version: ${observed.version}`);
  if (!observed.kernel || !observed.osRelease || !/^[a-f0-9]{64}$/.test(observed.packageInventorySha256)) {
    throw new Error(`Incomplete environment fingerprint: ${target.id}`);
  }
  if (observed.os === 'linux') {
    for (const [name, minimum] of Object.entries(contract.minimums.linux)) {
      if (!observed.dependencies?.[name] || !versionAtLeast(observed.dependencies[name], minimum)) {
        throw new Error(`Unsupported ${name} on ${target.id}: requires ${minimum}`);
      }
    }
  }
}

export function validateCodeCheckEnvironment(observed, targetId, contract) {
  const mac = contract.codeChecks.macos.find(target => target.id === targetId);
  const linux = ciMatrices(contract).linux.include.find(target => target.id === targetId);
  const distro = contract.codeChecks.linuxDistributions.find(item => item.id === linux?.distro);
  const expected = mac ?? (linux && { ...linux, os: 'linux', version: distro.version });
  if (!expected || observed.os !== expected.os || observed.arch !== expected.arch || observed.translated
    || (linux && observed.distro !== linux.distro)) throw new Error(`Wrong source CI host: ${targetId}`);
  const versionMatch = expected.version === 'rolling' ? 'rolling' : expected.version.includes('.') ? 'minor' : 'major';
  if (!matchesVersion(observed.version, { version: expected.version, versionMatch })) throw new Error(`Wrong source CI OS version: ${targetId}`);
}
