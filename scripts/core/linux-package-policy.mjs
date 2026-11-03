const MAX_LISTING_BYTES = 4 * 1024 * 1024;
const MAX_PACKAGE_CONTENT_BYTES = 1024 * 1024 * 1024;
const MAX_LISTING_ENTRIES = 50_000;
const MAX_TAR_STREAM_BYTES = MAX_PACKAGE_CONTENT_BYTES + MAX_LISTING_ENTRIES * 1024 + 1024;
const MAX_EXECUTABLE_BYTES = 256 * 1024 * 1024;
const MAX_ELF_OUTPUT_BYTES = 3 * MAX_EXECUTABLE_BYTES;
const MAX_STDERR_BYTES = 16 * 1024;
const PROCESS_TIMEOUT_MS = 120_000;


export const LINUX_PACKAGE_LIMITS = { MAX_LISTING_BYTES, MAX_PACKAGE_CONTENT_BYTES, MAX_LISTING_ENTRIES, MAX_TAR_STREAM_BYTES, MAX_EXECUTABLE_BYTES, MAX_ELF_OUTPUT_BYTES, MAX_STDERR_BYTES, PROCESS_TIMEOUT_MS };

const expectedElf = {
  amd64: { class: 2, machine: 62, name: 'x86-64' },
  arm64: { class: 2, machine: 183, name: 'AArch64' },
};

const requiredDependencies = [
  ['libpipewire-0.3-0t64'], ['liblilv-0-0'], ['libgcc-s1'], ['libc6'],
  ['libwebkit2gtk-4.1-0'], ['libgtk-3-0', 'libgtk-3-0t64'],
  ['librsvg2-common'],
  ['pipewire'], ['wireplumber'],
  ['libayatana-appindicator3-1', 'libappindicator3-1'],
  ['zenity'], ['pkexec'], ['polkit-1-auth-agent', 'lxpolkit'],
  ['systemd'], ['util-linux'],
];
// Release dependency floors use plain upstream versions, not distro epochs/revisions.
function satisfiesAudioMinimum(term, name, minimum) {
  const match = term.trim().match(/^([a-z0-9][a-z0-9+.-]*)\s+\((?:>=|>>|=)\s*(\d+)\.(\d+)\.(\d+)\)$/);
  if (!match || match[1] !== name) return false;
  const version = match.slice(2).map(Number);
  if (!version.every(Number.isSafeInteger)) return false;
  for (let index = 0; index < minimum.length; index++) {
    if (version[index] !== minimum[index]) return version[index] > minimum[index];
  }
  return true;
}
export const LINUX_EXECUTABLES = ['usr/bin/mixrelay-desktop', 'usr/bin/mixrelay-service', 'usr/bin/mixrelay-driver-service'];
const executables = LINUX_EXECUTABLES;
const wirePlumberFiles = [
  'usr/share/wireplumber/scripts/mixrelay-source-mute.lua',
  'usr/share/wireplumber/wireplumber.conf.d/90-mixrelay-source-mute.conf',
];
const uninstallFiles = [
  'usr/lib/mixrelay/uninstall.sh',
  'usr/share/applications/mixrelay-uninstall.desktop',
];
export const VERSION_RECEIPT = 'usr/share/mixrelay-driver/version.json';

export function packageMetadata({ architecture, depends, contents, packageName, homepage, maintainer }, { minimums, metadata, releaseDocuments }) {
// Async contributions need PipeWire 1.2; source muting needs WirePlumber 0.5.
const requiredAudioDependencies = [
  ...['libpipewire-0.3-0t64', 'pipewire'].map(name => [name, minimums.linux.pipewire.split('.').map(Number)]),
  ['wireplumber', minimums.linux.wireplumber.split('.').map(Number)],
];

const dataFiles = [
  ...releaseDocuments.map(filename => `usr/share/doc/mix-relay/${filename}`),
  'usr/lib/systemd/user/mixrelay-driver.service',
  'etc/xdg/autostart/mixrelay-driver.desktop',
  'usr/lib/mixrelay-driver/session-start.sh',
  'usr/lib/mixrelay-driver/before-change.sh',
  VERSION_RECEIPT,
  ...releaseDocuments.map(filename => `usr/share/doc/mix-relay-driver/${filename}`),
  'usr/share/doc/mix-relay-driver/third-party/truce-rack/LICENSE-MIT',
  'usr/share/doc/mix-relay-driver/third-party/truce-rack/LICENSE-APACHE',
];

  if (packageName !== 'mix-relay') throw new Error(`Unsupported package: ${packageName}`);
  if (homepage?.trim() !== metadata.homepage || maintainer?.trim() !== metadata.contact) {
    throw new Error('Package Homepage and Maintainer must match the public product metadata');
  }
  const normalizedArchitecture = architecture.trim();
  if (!Object.hasOwn(expectedElf, normalizedArchitecture)) {
    throw new Error(`Unsupported package architecture: ${normalizedArchitecture}`);
  }
  // A required library must occur in every alternative of at least one clause:
  // "required | unrelated" does not guarantee the runtime library is installed.
  const parseDependencyTerm = term => {
    const normalized = term.trim();
    if (/^[a-z0-9][a-z0-9+.-]*:[a-z0-9-]+(?:\s|$)/.test(normalized)
      || /\[[^\]]*\]|(?:^|\s)<[^>]*>/.test(normalized)) {
      throw new Error(`Unsupported dependency qualifier or restriction: ${normalized}`);
    }
    return normalized.match(/^([a-z0-9][a-z0-9+.-]*)(?:\s+\([^)]*\))?$/)?.[1];
  };
  const clauses = depends.split(',').map(clause => clause.split('|').map(parseDependencyTerm));
  for (const alternatives of requiredDependencies) {
    if (!clauses.some(clause => clause.length && clause.every(name => alternatives.includes(name)))) {
      throw new Error(`Missing required dependency: ${alternatives.join(' or ')}`);
    }
  }
  for (const [name, minimum] of requiredAudioDependencies) {
    if (!depends.split(',').some(clause => clause.split('|').every(term => satisfiesAudioMinimum(term, name, minimum)))) {
      throw new Error(`Missing required dependency: ${name} (>= ${minimum.join('.')})`);
    }
  }
  const files = new Map();
  let listingEntries = 0;
  let totalRegularBytes = 0;
  for (const line of contents.split('\n')) {
    if (!line) continue;
    const match = line.match(/^([-dl][rwxstST-]{9})\s+(\S+)\s+(\d+)\s+\S+\s+\S+\s+(.+)$/);
    if (!match) throw new Error(`Unrecognized Debian payload listing entry: ${line.slice(0, 200)}`);
    listingEntries += 1;
    if (listingEntries > MAX_LISTING_ENTRIES) throw new Error('Debian payload has too many entries');
    const size = Number(match[3]);
    if (!Number.isSafeInteger(size) || size < 0) throw new Error('Invalid Debian payload entry size');
    const displayedPath = match[4].split(' -> ', 1)[0];
    const path = displayedPath.replace(/^(?:\.\/)+/, '').replace(/\/+$/, '');
    if (!path) continue;
    if (path.startsWith('/') || path.includes('\0') || path.split('/').some(part => part === '.' || part === '..')) {
      throw new Error('Unsafe payload path');
    }
    if (files.has(path)) throw new Error(`Duplicate payload path: ${path}`);
    const entry = { mode: match[1], owner: match[2], size, memberPath: displayedPath };
    files.set(path, entry);
    if (entry.mode[0] === '-') {
      totalRegularBytes += size;
      if (!Number.isSafeInteger(totalRegularBytes) || totalRegularBytes > MAX_PACKAGE_CONTENT_BYTES) {
        throw new Error('Debian package payload exceeds the bounded inspection size');
      }
    }
  }
  const executableEntries = [];
  for (const path of executables) {
    const file = files.get(path);
    if (!file || file.size === 0 || file.mode[0] !== '-') {
      throw new Error(`Missing non-empty regular executable payload: ${path}`);
    }
    if (file.owner !== '0/0' || file.mode !== '-rwxr-xr-x') {
      throw new Error(`Invalid Debian executable ownership or permissions: ${path}`);
    }
    if (file.size > MAX_EXECUTABLE_BYTES) {
      throw new Error(`Executable payload exceeds the bounded inspection size: ${path}`);
    }
    executableEntries.push({ path, size: file.size, memberPath: file.memberPath });
  }
  for (const path of wirePlumberFiles) {
    const file = files.get(path);
    if (!file || file.size === 0 || file.mode[0] !== '-') {
      throw new Error(`Missing non-empty regular WirePlumber payload: ${path}`);
    }
    if (file.owner !== '0/0' || file.mode !== '-rw-r--r--') {
      throw new Error(`Invalid WirePlumber payload ownership or permissions: ${path}`);
    }
  }
  for (const path of uninstallFiles) {
    const file = files.get(path);
    if (!file || file.size === 0 || file.mode !== '-rw-r--r--' || file.owner !== '0/0') {
      throw new Error(`Missing or invalid uninstall payload: ${path}`);
    }
  }
  for (const path of dataFiles) {
    const file = files.get(path);
    if (!file || file.size === 0 || file.mode !== '-rw-r--r--' || file.owner !== '0/0') {
      throw new Error(`Missing or invalid package documentation: ${path}`);
    }
  }
  return { architecture: normalizedArchitecture, files, executableEntries };
}

function elfHeaderArchitecture(header, path) {
  if (!(header instanceof Uint8Array) || header.byteLength < 64
    || header[0] !== 0x7f || header[1] !== 0x45 || header[2] !== 0x4c || header[3] !== 0x46) {
    throw new Error(`Payload is not a complete ELF64 executable: ${path}`);
  }
  if (header[4] !== 2 || header[5] !== 1 || header[6] !== 1) {
    throw new Error(`Payload has an unsupported ELF class, byte order, or version: ${path}`);
  }
  const type = header[16] | (header[17] << 8);
  const machine = header[18] | (header[19] << 8);
  const headerSize = header[52] | (header[53] << 8);
  if (![2, 3].includes(type) || headerSize < 64) {
    throw new Error(`Payload does not have a supported ELF executable header: ${path}`);
  }
  return { machine, name: machine === 62 ? 'x86-64' : machine === 183 ? 'AArch64' : `e_machine=${machine}` };
}

/** Checks metadata and ELF architecture without executing payloads. */
export function validatePackage({ architecture, depends, contents, elfHeaders, packageName, version, versionReceipt, homepage, maintainer }, policy) {
  const { architecture: normalizedArchitecture, executableEntries } = packageMetadata({ architecture, depends, contents, packageName, homepage, maintainer }, policy);
  const receipt = JSON.parse(versionReceipt ?? 'null');
  if (!receipt || !/^\d+\.\d+\.\d+$/.test(receipt.version) || receipt.version !== version?.replace(/-\d+$/, '')) {
    throw new Error('Driver version receipt must match the actual package version');
  }
  const expected = expectedElf[normalizedArchitecture];
  const elfArchitectures = {};
  for (const entry of executableEntries) {
    const header = elfHeaders?.[entry.path];
    if (!header) throw new Error(`Missing extracted ELF header: ${entry.path}`);
    const actual = elfHeaderArchitecture(header, entry.path);
    if (actual.machine !== expected.machine) {
      throw new Error(`Unexpected ELF architecture for ${entry.path}: expected ${expected.name}, found ${actual.name}`);
    }
    elfArchitectures[entry.path] = actual.name;
  }
  return {
    packageName,
    architecture: normalizedArchitecture,
    executables,
    elfArchitectures,
    scope: 'metadata-payload-elf',
  };
}
