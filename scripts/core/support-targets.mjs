export function ciMatrices(contract) {
  return {
    macos: { include: contract.codeChecks.macos },
    linux: { include: contract.codeChecks.linuxDistributions.flatMap(distro =>
      distro.architectures.map(arch => ({ id: `${distro.id}-${arch}`, distro: distro.id, arch,
        runner: arch === 'arm64' ? 'ubuntu-24.04-arm' : 'ubuntu-24.04' }))) },
    linuxBuild: { include: contract.artifacts.filter(a => a.os === 'linux' && a.format === 'deb').map(artifact => ({
      id: artifact.id, arch: artifact.arch, distro: contract.build.linuxDistribution,
      runner: artifact.arch === 'arm64' ? 'ubuntu-24.04-arm' : 'ubuntu-24.04',
    })) },
  };
}


export function matchesVersion(actual, target) {
  if (target.versionMatch === 'rolling') return typeof actual === 'string' && actual.length > 0;
  const width = target.versionMatch === 'major' ? 1 : 2;
  return actual.split('.').slice(0, width).join('.') === target.version;
}


export function requiredScenarios(contract, target) {
  return contract.scenarios.filter(item => item.required && (item.artifact
    ? item.artifact === target.artifact
    : !target.kind && (!item.os || item.os.includes(target.os))));
}
