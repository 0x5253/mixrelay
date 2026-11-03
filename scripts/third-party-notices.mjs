/**
 * Generates the third-party notice file bundled with MixRelay.
 *
 * Rust: normal (non-dev, non-build) dependencies of every shipped workspace
 * artifact on each release target. JavaScript: the production dependency
 * closure of the control UI. Each distinct licence text is written once.
 *
 *   node scripts/third-party-notices.mjs sync   # rewrite the committed file
 *   node scripts/third-party-notices.mjs check  # fail when it is stale
 */
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, readdirSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const root = fileURLToPath(new URL('..', import.meta.url));
export const noticesPath = resolve(root, 'third-party/THIRD-PARTY-NOTICES.txt');
export const RELEASE_TARGETS = [
  'aarch64-apple-darwin',
  'x86_64-apple-darwin',
  'x86_64-unknown-linux-gnu',
  'aarch64-unknown-linux-gnu',
];
const SHIPPED_KINDS = new Set(['bin', 'cdylib', 'staticlib']);
const LICENSE_FILE = /^(?:licen[cs]e|copying|notice|copyright|unlicense)(?:[.-].*)?$/i;

const normalize = text => `${text.replace(/\r\n?/g, '\n').replace(/\s+$/, '')}\n`;

export function licenseTexts(directory, extraFile) {
  const names = existsSync(directory)
    ? readdirSync(directory, { withFileTypes: true })
      .filter(entry => entry.isFile() && LICENSE_FILE.test(entry.name))
      .map(entry => join(directory, entry.name))
    : [];
  if (extraFile && existsSync(extraFile) && !names.includes(extraFile)) names.push(extraFile);
  return names.sort().map(path => normalize(readFileSync(path, 'utf8')));
}

/** Third-party Rust packages reachable through normal dependencies of shipped artifacts. */
export function rustPackages(metadata) {
  const members = new Set(metadata.workspace_members);
  const byId = new Map(metadata.packages.map(item => [item.id, item]));
  const nodes = new Map(metadata.resolve.nodes.map(node => [node.id, node]));
  const pending = metadata.workspace_members.filter(id =>
    byId.get(id).targets.some(target => target.kind.some(kind => SHIPPED_KINDS.has(kind))));
  const seen = new Set(pending);
  while (pending.length > 0) {
    for (const dep of nodes.get(pending.pop())?.deps ?? []) {
      if (seen.has(dep.pkg) || !dep.dep_kinds.some(kind => kind.kind === null)) continue;
      seen.add(dep.pkg);
      pending.push(dep.pkg);
    }
  }
  return [...seen].filter(id => !members.has(id)).map(id => {
    const item = byId.get(id);
    const directory = dirname(item.manifest_path);
    return {
      name: item.name,
      version: item.version,
      license: item.license ?? 'see licence text',
      texts: licenseTexts(directory, item.license_file && resolve(directory, item.license_file)),
    };
  });
}

function findPackage(name, from) {
  for (let directory = from; ; directory = dirname(directory)) {
    const candidate = join(directory, 'node_modules', name);
    if (existsSync(join(candidate, 'package.json'))) return realpathSync(candidate);
    if (dirname(directory) === directory) return undefined;
  }
}

/** Production dependency closure of an npm package, resolved through node_modules. */
export function javascriptPackages(packageDirectory) {
  const found = new Map();
  const visit = directory => {
    const manifest = JSON.parse(readFileSync(join(directory, 'package.json'), 'utf8'));
    for (const name of Object.keys({ ...manifest.dependencies, ...manifest.optionalDependencies })) {
      const dependency = findPackage(name, directory);
      if (!dependency || found.has(dependency)) continue;
      const own = JSON.parse(readFileSync(join(dependency, 'package.json'), 'utf8'));
      const license = typeof own.license === 'string' ? own.license : own.license?.type;
      found.set(dependency, { name: own.name, version: own.version, license: license ?? 'see licence text', texts: licenseTexts(dependency) });
      visit(dependency);
    }
  };
  visit(realpathSync(packageDirectory));
  return [...found.values()];
}

function uniquePackages(packages) {
  const unique = new Map();
  for (const item of packages) unique.set(`${item.name}@${item.version}`, item);
  return [...unique.values()].sort((a, b) => a.name.localeCompare(b.name, 'en') || a.version.localeCompare(b.version, 'en'));
}

export function renderNotices({ rust, javascript }) {
  const groups = new Map();
  const missing = [];
  const listing = (title, packages) => [
    `${title} (${packages.length})`,
    ...packages.map(item => {
      if (item.texts.length === 0) missing.push(item);
      for (const text of item.texts) {
        const key = createHash('sha256').update(text).digest('hex');
        if (!groups.has(key)) groups.set(key, { text, users: new Set() });
        groups.get(key).users.add(`${item.name} ${item.version}`);
      }
      return `- ${item.name} ${item.version} (${item.license})`;
    }),
    '',
  ];
  const lines = [
    'MixRelay third-party notices',
    'Generated by scripts/third-party-notices.mjs. Do not edit by hand.',
    'MixRelay includes the following components under their own licences.',
    'Source code for each listed version is available from crates.io or npm.',
    '',
    ...listing('Rust crates', uniquePackages(rust)),
    ...listing('JavaScript packages', uniquePackages(javascript)),
  ];
  if (missing.length > 0) {
    lines.push('Packages that ship no separate licence file; their licence is the SPDX expression listed above:',
      ...missing.map(item => `- ${item.name} ${item.version}`), '');
  }
  const ordered = [...groups.values()]
    .map(group => ({ ...group, users: [...group.users].sort((a, b) => a.localeCompare(b, 'en')) }))
    .sort((a, b) => a.users[0].localeCompare(b.users[0], 'en'));
  for (const group of ordered) {
    lines.push('='.repeat(78), `Applies to: ${group.users.join(', ')}`, '-'.repeat(78), group.text.trimEnd(), '');
  }
  return `${lines.join('\n').trimEnd()}\n`;
}

export function generateNotices(directory = root) {
  const rust = RELEASE_TARGETS.flatMap(target => rustPackages(JSON.parse(execFileSync('cargo',
    ['metadata', '--format-version', '1', '--locked', '--filter-platform', target],
    { cwd: directory, encoding: 'utf8', maxBuffer: 512 * 1024 * 1024 }))));
  return renderNotices({ rust, javascript: javascriptPackages(resolve(directory, 'apps/control-ui')) });
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const mode = process.argv[2];
  const generated = generateNotices();
  if (mode === 'sync') {
    writeFileSync(noticesPath, generated);
    console.log(`Wrote ${noticesPath}`);
  } else if (mode === 'check') {
    if (!existsSync(noticesPath) || readFileSync(noticesPath, 'utf8') !== generated) {
      console.error('Third-party notices are stale. Run: pnpm notices:sync');
      process.exitCode = 1;
    } else {
      console.log('Third-party notices are current.');
    }
  } else {
    console.error('Usage: node scripts/third-party-notices.mjs <sync|check>');
    process.exitCode = 2;
  }
}
