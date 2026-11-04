import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { lstatSync, mkdtempSync, readdirSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { inspectPackage } from './check-linux-package.mjs';
import { loadContract } from './support-contract.mjs';

const rpmArchitecture = { amd64: 'x86_64', arm64: 'aarch64' };
const scriptlets = {
  PREIN: 'driver-before-change.sh', PREUN: 'driver-rpm-pre-remove.sh',
  POSTIN: 'driver-after-install.sh', POSTUN: 'driver-after-remove.sh',
};
const fileFormat = '[%{FILEMODES:perms}\\t%{FILEUSERNAME}:%{FILEGROUPNAME}\\t%{FILEDIGESTS}\\t%{FILENAMES}\\n]';

function tool(program, args) {
  const result = spawnSync(program, args, {
    encoding: 'utf8', env: { ...process.env, LC_ALL: 'C', LANG: 'C' }, timeout: 120_000, maxBuffer: 16 * 1024 * 1024,
  });
  if (result.error || result.status !== 0 || result.signal) {
    throw new Error(`${program} ${args[0]} failed: ${result.error?.message || result.stderr}`);
  }
  return result.stdout;
}

/** Regular files and symlinks of an RPM header listing, keyed by path without the leading slash. */
export function rpmFiles(listing) {
  const files = new Map();
  for (const line of listing.split('\n')) {
    if (!line) continue;
    const [mode, owner, digest, path] = line.split('\t');
    if (!path?.startsWith('/') || files.has(path.slice(1))) throw new Error(`Invalid RPM file entry: ${line.slice(0, 200)}`);
    if (mode[0] === 'd') continue;
    if (owner !== 'root:root') throw new Error(`RPM file is not owned by root: ${path}`);
    files.set(path.slice(1), { mode, digest });
  }
  return files;
}

/** The DEB's regular files and symlinks with modes and SHA-256 digests, as extracted by dpkg-deb. */
export function extractedFiles(directory) {
  const files = new Map();
  for (const entry of readdirSync(directory, { recursive: true })) {
    const path = join(directory, entry);
    const stat = lstatSync(path);
    if (stat.isDirectory()) continue;
    if (!stat.isFile() && !stat.isSymbolicLink()) throw new Error(`Unsupported DEB payload entry: ${entry}`);
    const type = stat.isSymbolicLink() ? 'l' : '-';
    const permissions = [6, 3, 0].map(shift => {
      const bits = (stat.mode >> shift) & 7;
      return `${bits & 4 ? 'r' : '-'}${bits & 2 ? 'w' : '-'}${bits & 1 ? 'x' : '-'}`;
    }).join('');
    files.set(entry, { mode: `${type}${permissions}`,
      digest: type === 'l' ? '' : createHash('sha256').update(readFileSync(path)).digest('hex') });
  }
  return files;
}

/** Fails unless the RPM is the checked DEB's payload with RPM metadata and lifecycle scripts. */
export function compareRpmToDeb({ metadata, requires, scripts, rpm, deb }, { deb: inspected, homepage, dependencies, source }) {
  const expected = {
    name: 'mix-relay', version: inspected.version.replace(/-\d+$/, ''), release: '1',
    arch: rpmArchitecture[inspected.architecture], url: homepage,
  };
  for (const [field, value] of Object.entries(expected)) {
    if (metadata[field] !== value) throw new Error(`RPM ${field} is ${metadata[field]}, expected ${value}`);
  }
  for (const dependency of dependencies) {
    if (!requires.includes(dependency)) throw new Error(`Missing RPM requirement: ${dependency}`);
  }
  for (const [tag, file] of Object.entries(scriptlets)) {
    if (scripts[tag]?.trim() !== source(file).trim()) throw new Error(`Missing or mismatched RPM scriptlet: ${tag}`);
  }
  const paths = [...new Set([...rpm.keys(), ...deb.keys()])].sort();
  for (const path of paths) {
    const [left, right] = [rpm.get(path), deb.get(path)];
    if (!left || !right) throw new Error(`${left ? 'DEB' : 'RPM'} payload lacks ${path}`);
    if (left.mode !== right.mode || left.digest !== right.digest) throw new Error(`RPM payload differs from the DEB: ${path}`);
  }
  return { ...expected, files: paths.length };
}

export async function inspectRpm(rpmPath, debPath) {
  const rpm = resolve(rpmPath);
  const deb = resolve(debPath);
  const inspected = await inspectPackage(deb);
  const { contract } = loadContract();
  const linux = JSON.parse(readFileSync(new URL('../apps/desktop-shell/src-tauri/tauri.linux.conf.json', import.meta.url)));
  // Header and payload digests must verify before the header is trusted.
  tool('rpm', ['--checksig', '--nosignature', rpm]);
  const [name, version, release, arch, url] = tool('rpm', ['-qp', '--queryformat',
    '%{NAME}\\n%{VERSION}\\n%{RELEASE}\\n%{ARCH}\\n%{URL}\\n', rpm]).split('\n');
  const scripts = Object.fromEntries(Object.keys(scriptlets)
    .map(tag => [tag, tool('rpm', ['-qp', '--queryformat', `%{${tag}}`, rpm])]));
  const directory = mkdtempSync(join(tmpdir(), 'mixrelay-rpm-check-'));
  try {
    tool('dpkg-deb', ['--extract', deb, directory]);
    return compareRpmToDeb({
      metadata: { name, version, release, arch, url },
      requires: tool('rpm', ['-qp', '--requires', rpm]).split('\n').map(line => line.trim()),
      scripts,
      rpm: rpmFiles(tool('rpm', ['-qp', '--queryformat', fileFormat, rpm])),
      deb: extractedFiles(directory),
    }, {
      deb: inspected, homepage: contract.metadata.homepage, dependencies: linux.bundle.linux.rpm.depends,
      source: file => readFileSync(new URL(`../packaging/linux/${file}`, import.meta.url), 'utf8'),
    });
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    if (process.argv.length !== 4) throw new Error('Usage: node scripts/check-rpm-package.mjs PACKAGE.rpm CHECKED.deb');
    console.log(JSON.stringify(await inspectRpm(process.argv[2], process.argv[3]), null, 2));
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
