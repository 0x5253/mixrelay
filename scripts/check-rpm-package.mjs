import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { lstatSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync } from 'node:fs';
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
const fileFormat = '[%{FILEMODES:perms}\\t%{FILEUSERNAME}:%{FILEGROUPNAME}\\t%{FILENAMES}\\n]';
// Tauri records the package type in each bundle's copy of the desktop binary.
const BUNDLE_MARKER = Buffer.from('__TAURI_BUNDLE_TYPE_VAR_');
const NEUTRAL_MARKER = Buffer.from('__TAURI_BUNDLE_TYPE_VAR_UNK');
const DESKTOP = 'usr/bin/mixrelay-desktop';

function tool(program, args) {
  const result = spawnSync(program, args, {
    encoding: 'utf8', env: { ...process.env, LC_ALL: 'C', LANG: 'C' }, timeout: 120_000, maxBuffer: 16 * 1024 * 1024,
  });
  if (result.error || result.status !== 0 || result.signal) {
    throw new Error(`${program} ${args[0]} failed: ${result.error?.message || result.stderr}`);
  }
  return result.stdout;
}

function addEntry(files, path, mode, owner, line) {
  if (!path || path.startsWith('/') || files.has(path)) throw new Error(`Invalid package file entry: ${line.slice(0, 200)}`);
  if (mode[0] === 'd') return;
  if (owner !== 'root:root' && owner !== '0:0') throw new Error(`Package file is not owned by root: ${path}`);
  files.set(path, mode);
}

/** Modes of an RPM header listing's files and links, keyed by path without the leading slash. */
export function rpmModes(listing) {
  const files = new Map();
  for (const line of listing.split('\n')) {
    if (!line) continue;
    const [mode, owner, path] = line.split('\t');
    addEntry(files, path?.startsWith('/') ? path.slice(1) : undefined, mode, owner, line);
  }
  return files;
}

/** Modes of a `dpkg-deb --contents` listing's files and links, keyed like `rpmModes`. */
export function debModes(listing) {
  const files = new Map();
  for (const line of listing.split('\n')) {
    if (!line) continue;
    // dpkg-deb shows names or numeric IDs, with or without a leading "./", per archive.
    const match = line.match(/^([-dl][rwxstST-]{9})\s+(\S+)\/(\S+)\s+\d+\s+\S+\s+\S+\s+(?:\.\/)?(.*?)(?: -> .*)?$/);
    if (!match) throw new Error(`Unrecognized DEB listing entry: ${line.slice(0, 200)}`);
    if (match[4] === '' || match[4] === '.') continue;
    addEntry(files, match[4].replace(/\/$/, ''), match[1], `${match[2]}:${match[3]}`, line);
  }
  return files;
}

/** One file's bytes with Tauri's package-type marker neutralised, and how many markers it held. */
export function normalizedContent(bytes, packageType) {
  const marker = Buffer.concat([BUNDLE_MARKER, Buffer.from(packageType)]);
  const content = Buffer.from(bytes);
  let markers = 0;
  for (let index = content.indexOf(marker); index !== -1; index = content.indexOf(marker, index + marker.length)) {
    NEUTRAL_MARKER.copy(content, index);
    markers += 1;
  }
  return { digest: createHash('sha256').update(content).digest('hex'), markers };
}

/** Normalised digests of an extracted payload's regular files, keyed by relative path. */
export function payloadDigests(directory, packageType) {
  const files = new Map();
  for (const entry of readdirSync(directory, { recursive: true })) {
    const path = join(directory, entry);
    if (lstatSync(path).isFile()) files.set(entry, normalizedContent(readFileSync(path), packageType));
  }
  return files;
}

/** Fails unless the RPM is the checked DEB's payload with RPM metadata and lifecycle scripts. */
export function compareRpmToDeb({ metadata, requires, scripts, modes, digests }, { version, architecture, homepage, dependencies, source }) {
  const expected = { name: 'mix-relay', version, release: '1', arch: rpmArchitecture[architecture], url: homepage };
  for (const [field, value] of Object.entries(expected)) {
    if (metadata[field] !== value) throw new Error(`RPM ${field} is ${metadata[field]}, expected ${value}`);
  }
  for (const dependency of dependencies) {
    if (!requires.includes(dependency)) throw new Error(`Missing RPM requirement: ${dependency}`);
  }
  for (const [tag, file] of Object.entries(scriptlets)) {
    if (scripts[tag]?.trim() !== source(file).trim()) throw new Error(`Missing or mismatched RPM scriptlet: ${tag}`);
  }
  const paths = [...new Set([...modes.rpm.keys(), ...modes.deb.keys()])].sort();
  for (const path of paths) {
    if (!modes.rpm.has(path) || !modes.deb.has(path)) throw new Error(`${modes.rpm.has(path) ? 'DEB' : 'RPM'} payload lacks ${path}`);
    const [rpm, deb] = [digests.rpm.get(path), digests.deb.get(path)];
    if (modes.rpm.get(path) !== modes.deb.get(path) || rpm?.digest !== deb?.digest || rpm?.markers !== deb?.markers)
      throw new Error(`RPM payload differs from the DEB: ${path}`);
  }
  if (digests.rpm.get(DESKTOP)?.markers !== 1) throw new Error('The RPM desktop binary must record its package type once');
  return { ...expected, files: paths.length };
}

export async function inspectRpm(rpmPath, debPath) {
  const rpm = resolve(rpmPath);
  const deb = resolve(debPath);
  const { architecture } = await inspectPackage(deb);
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
    const [debRoot, rpmRoot] = [join(directory, 'deb'), join(directory, 'rpm')];
    mkdirSync(rpmRoot);
    tool('dpkg-deb', ['--extract', deb, debRoot]);
    tool('bsdtar', ['-xf', rpm, '-C', rpmRoot]);
    return compareRpmToDeb({
      metadata: { name, version, release, arch, url },
      requires: tool('rpm', ['-qp', '--requires', rpm]).split('\n').map(line => line.trim()),
      scripts,
      modes: { rpm: rpmModes(tool('rpm', ['-qp', '--queryformat', fileFormat, rpm])), deb: debModes(tool('dpkg-deb', ['--contents', deb])) },
      digests: { rpm: payloadDigests(rpmRoot, 'RPM'), deb: payloadDigests(debRoot, 'DEB') },
    }, {
      version: tool('dpkg-deb', ['--field', deb, 'Version']).trim().replace(/-\d+$/, ''),
      architecture, homepage: contract.metadata.homepage, dependencies: linux.bundle.linux.rpm.depends,
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
