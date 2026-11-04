// Makes a lab distribution's base image available to `docker compose build`. Registry
// images need nothing; a distribution published only as a rootfs is downloaded, checked
// against its pinned digest and imported under the tag the compose file names.
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { createReadStream, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { loadContract, rootfsImage } from '../support-contract.mjs';

function docker(args, options = {}) {
  const result = spawnSync('docker', args, { stdio: options.quiet ? 'ignore' : 'inherit' });
  if (result.error) throw result.error;
  return result.status === 0;
}

async function sha256(path) {
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  return hash.digest('hex');
}

async function download(url, path) {
  const result = spawnSync('curl', ['--fail', '--location', '--retry', '5', '--silent', '--show-error',
    '--output', path, url], { stdio: 'inherit' });
  if (result.error || result.status !== 0) throw new Error(`Download failed: ${url}`);
}

export async function prepareDistroImage(id, contract = loadContract().contract) {
  const distro = contract.codeChecks.linuxDistributions.find(candidate => candidate.id === id);
  if (!distro) throw new Error(`Unknown lab distribution: ${id}`);
  if (!distro.rootfs) return distro.image;
  const tag = rootfsImage(distro);
  if (docker(['image', 'inspect', tag], { quiet: true })) return tag;
  const directory = mkdtempSync(join(tmpdir(), 'mixrelay-rootfs-'));
  try {
    const archive = join(directory, 'rootfs.tar');
    await download(distro.rootfs.url, archive);
    if (await sha256(archive) !== distro.rootfs.digest) throw new Error(`Rootfs digest differs from the contract: ${id}`);
    const platform = { x64: 'linux/amd64', arm64: 'linux/arm64' }[distro.architectures[0]];
    if (!docker(['import', '--platform', platform, archive, tag])) throw new Error(`docker import failed: ${id}`);
    return tag;
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  const id = process.argv[2];
  if (!id || process.argv.length !== 3) {
    console.error('Usage: pnpm lab:distro-image DISTRIBUTION');
    process.exitCode = 1;
  } else {
    prepareDistroImage(id).then(image => console.log(image))
      .catch(error => { console.error(error.message); process.exitCode = 1; });
  }
}
