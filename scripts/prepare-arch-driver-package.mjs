import { renderArchRecipe as renderRecipe } from './core/arch-recipe.mjs';
import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import * as fs from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { inspectPackage } from './check-linux-package.mjs';
import { loadContract } from './support-contract.mjs';
import { productContact } from './core/product-metadata-policy.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');

function runTool(program, args) {
  const result = spawnSync(program, args, { encoding: 'utf8', timeout: 120_000 });
  if (result.error || result.status !== 0) {
    throw new Error(`${program} failed: ${result.error?.message || result.stderr}`);
  }
  return result.stdout.trim();
}

export function renderArchRecipe(values) {
  const { contract } = loadContract();
  return renderRecipe(values, { template: fs.readFileSync(join(root, 'packaging/linux/arch/PKGBUILD.in'), 'utf8'),
    metadata: { ...contract.metadata, contact: productContact(contract.metadata) }, minimums: contract.minimums });
}

/** Prepare a native makepkg recipe from the validated common release payload. */
export async function prepareArchPackage({ deb, output }) {
  const archive = resolve(deb);
  const destination = resolve(output);
  if (!fs.lstatSync(archive).isFile()) throw new Error('DEB input must be a regular file');
  if (fs.existsSync(destination)) throw new Error('Arch build directory must not already exist');
  const inspected = await inspectPackage(archive);
  if (inspected.packageName !== 'mix-relay') throw new Error('Expected the complete MixRelay DEB');
  const arch = { amd64: 'x86_64', arm64: 'aarch64' }[inspected.architecture];
  const version = runTool('dpkg-deb', ['--field', archive, 'Version']).replace(/-\d+$/, '');
  // Validate all shell substitutions before creating a build directory.
  renderArchRecipe({ version, architecture: inspected.architecture, checksum: '0'.repeat(64) });
  fs.mkdirSync(dirname(destination), { recursive: true });
  const staging = fs.mkdtempSync(join(dirname(destination), '.arch-driver-'));
  try {
    const payload = join(staging, 'payload');
    runTool('dpkg-deb', ['--extract', archive, payload]);
    const support = join(payload, 'usr/lib/mixrelay-driver');
    fs.copyFileSync(join(root, 'packaging/linux/driver-after-install.sh'), join(support, 'after-install.sh'));
    const hooks = join(payload, 'usr/share/libalpm/hooks');
    fs.mkdirSync(hooks, { recursive: true });
    fs.copyFileSync(join(root, 'packaging/linux/arch/mixrelay-driver.hook'), join(hooks, 'mixrelay-driver.hook'));
    const licenseDir = join(payload, 'usr/share/licenses/mix-relay');
    fs.mkdirSync(licenseDir, { recursive: true });
    fs.copyFileSync(join(payload, 'usr/share/doc/mix-relay-driver/LICENSE.txt'), join(licenseDir, 'LICENSE'));
    const payloadName = `mix-relay-${version}-${arch}.tar.gz`;
    const payloadArchive = join(staging, payloadName);
    runTool('tar', ['-czf', payloadArchive, '-C', staging, 'payload']);
    const checksum = createHash('sha256').update(fs.readFileSync(payloadArchive)).digest('hex');
    fs.writeFileSync(join(staging, 'PKGBUILD'), renderArchRecipe({ version, architecture: inspected.architecture, checksum }));
    // Removal cannot execute a script from the just-removed package tree.
    const afterRemove = fs.readFileSync(join(root, 'packaging/linux/driver-after-remove.sh'), 'utf8')
      .replace(/^#![^\n]*\n/, '').replace(/^set -eu\n/m, '');
    fs.writeFileSync(join(staging, 'mixrelay.install'),
      `post_install() { /bin/sh /usr/lib/mixrelay-driver/after-install.sh; }\n` +
      `post_upgrade() { post_install; }\npost_remove() {\n${afterRemove}\n}\n`);
    fs.writeFileSync(join(staging, 'release-input.json'), `${JSON.stringify({
      version, arch, debSha256: createHash('sha256').update(fs.readFileSync(archive)).digest('hex'),
      payloadSha256: checksum,
    }, null, 2)}\n`);
    fs.rmSync(payload, { recursive: true });
    fs.renameSync(staging, destination);
    return { version, arch, destination };
  } finally {
    fs.rmSync(staging, { recursive: true, force: true });
  }
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  const [deb, output] = process.argv.slice(2);
  if (!deb || !output || process.argv.length !== 4) {
    console.error('Usage: node scripts/prepare-arch-driver-package.mjs INPUT.deb NEW_BUILD_DIRECTORY');
    process.exitCode = 1;
  } else {
    prepareArchPackage({ deb, output }).then(result => console.log(JSON.stringify(result)))
      .catch(error => { console.error(error.message); process.exitCode = 1; });
  }
}
