export function ciMatrices(contract) {
  return {
    macos: { include: contract.codeChecks.macos },
    // Pull requests build the oldest and newest supported macOS on each CPU; the full set
    // runs on manual dispatch and in installed qualification of release candidates.
    macosPullRequest: { include: contract.codeChecks.macos.filter(target => target.pullRequest) },
    linux: { include: contract.codeChecks.linuxDistributions.flatMap(distro =>
      distro.architectures.map(arch => ({ id: `${distro.id}-${arch}`, distro: distro.id, arch, runner: runners[arch] }))) },
    // One build per CPU bundles every Tauri package format from the same binaries;
    // the Arch package later wraps that CPU's checked DEB payload.
    linuxBuild: { include: linuxArtifacts(contract, 'deb').map(artifact => {
      const rpm = linuxArtifacts(contract, 'rpm').find(a => a.arch === artifact.arch);
      const arch = linuxArtifacts(contract, 'arch').find(a => a.arch === artifact.arch);
      return { id: artifact.id, arch: artifact.arch, distro: contract.build.linuxDistribution,
        runner: runners[artifact.arch], bundles: rpm ? 'deb,rpm' : 'deb', rpm: rpm?.id ?? '', archPackage: arch?.id ?? '' };
    }) },
    // makepkg runs natively in the Arch distribution of the package's CPU.
    archBuild: { include: linuxArtifacts(contract, 'arch').map(artifact => ({
      id: artifact.id, arch: artifact.arch, runner: runners[artifact.arch], pkgarch: pacmanArchitectures[artifact.arch],
      distro: contract.codeChecks.linuxDistributions.find(d => d.artifactFormat === 'arch' && d.architectures.includes(artifact.arch)).id,
    })) },
  };
}

const runners = { x64: 'ubuntu-24.04', arm64: 'ubuntu-24.04-arm' };
const pacmanArchitectures = { x64: 'x86_64', arm64: 'aarch64' };
const linuxArtifacts = (contract, format) => contract.artifacts.filter(a => a.os === 'linux' && a.format === format);


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
