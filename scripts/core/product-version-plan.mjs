export const PRODUCT_JSON_MANIFESTS = ['package.json', 'apps/control-ui/package.json', 'apps/desktop-shell/package.json', 'apps/desktop-shell/src-tauri/tauri.conf.json'];
export const HAL_MANIFEST = 'native/macos/hal-driver/driver/Info.plist';
// `cargo build --locked` rejects a lockfile whose workspace crate versions differ from their manifests.
// Path packages have no `source` line; registry and git packages keep their own versions.
const lockedPackage = /^\[\[package\]\]\nname = "([^"\n]+)"\nversion = "([^"\n]+)"\n(?!source = )/gm;
export function productVersionFrom(cargo) {
  const version = cargo.match(/\[workspace.package\]\s*\nversion = "([^"\n]+)"/)?.[1];
  if (!version || !/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(version)) throw new Error('Cargo workspace must define a stable product version');
  return version;
}
export function workspaceMembersFrom(cargo) {
  const members = cargo.match(/^members = \[([^\]]*)\]/m)?.[1];
  if (!members) throw new Error('Cargo workspace must list its members');
  return [...members.matchAll(/"([^"\n]+)"/g)].map(([, member]) => member);
}
// Crates that inherit the product version. Vendored crates and fixtures keep their own versions.
export function productCratesFrom(sources) {
  return workspaceMembersFrom(sources.get('Cargo.toml')).flatMap(member => {
    const manifest = sources.get(`${member}/Cargo.toml`);
    const section = manifest.match(/^\[package\]\n((?:(?!\[)[^\n]*(?:\n|$))*)/m)?.[1] ?? '';
    if (!/^version(?:\.workspace\s*=\s*true|\s*=\s*\{\s*workspace\s*=\s*true\s*\})\s*$/m.test(section)) return [];
    const name = section.match(/^name = "([^"\n]+)"/m)?.[1];
    if (!name) throw new Error(`${member}/Cargo.toml must name its package`);
    return [name];
  });
}
function lockedVersions(lock) {
  return new Map([...lock.matchAll(lockedPackage)].map(([, name, version]) => [name, version]));
}
function updatedLock(lock, crates, version) {
  const locked = lockedVersions(lock);
  const missing = crates.filter(name => !locked.has(name));
  if (missing.length) throw new Error(`Cargo.lock is missing workspace crates: ${missing.join(', ')}`);
  const names = new Set(crates);
  return lock.replace(lockedPackage, (entry, name) => names.has(name) ? `[[package]]\nname = "${name}"\nversion = "${version}"\n` : entry);
}
export function checkVersionSnapshot(sources) {
  const version = productVersionFrom(sources.get('Cargo.toml'));
  for (const filename of PRODUCT_JSON_MANIFESTS) {
    if (JSON.parse(sources.get(filename)).version !== version) throw new Error(`${filename} differs from MixRelay ${version}`);
  }
  const hal = sources.get(HAL_MANIFEST);
  for (const key of ['CFBundleShortVersionString', 'CFBundleVersion']) {
    if (hal.match(new RegExp(`<key>${key}</key>\\s*<string>([^<]+)</string>`))?.[1] !== version) throw new Error(`${HAL_MANIFEST} ${key} differs from MixRelay ${version}`);
  }
  const locked = lockedVersions(sources.get('Cargo.lock'));
  for (const name of productCratesFrom(sources)) {
    if (locked.get(name) !== version) throw new Error(`Cargo.lock ${name} differs from MixRelay ${version}`);
  }
  return version;
}
export function planVersion(sources, version) {
  if (!/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(version)) throw new Error('Use stable MAJOR.MINOR.PATCH');
  const values = PRODUCT_JSON_MANIFESTS.map((name) => JSON.parse(sources.get(name)));
  const cargo = sources.get('Cargo.toml');
  productVersionFrom(cargo);
  let hal = sources.get(HAL_MANIFEST);
  for (const key of ['CFBundleShortVersionString', 'CFBundleVersion']) {
    if (!new RegExp(`<key>${key}</key>\\s*<string>[^<]+</string>`).test(hal)) throw new Error(`${HAL_MANIFEST} is missing ${key}`);
    hal = hal.replace(new RegExp(`(<key>${key}</key>\\s*<string>)[^<]+`), `$1${version}`);
  }
  const lock = updatedLock(sources.get('Cargo.lock'), productCratesFrom(sources), version);
  const edits = new Map([
    ['Cargo.toml', cargo.replace(/(\[workspace.package\]\s*\nversion = ")[^"\n]+/, `$1${version}`)],
    [HAL_MANIFEST, hal], ['Cargo.lock', lock],
    ...PRODUCT_JSON_MANIFESTS.map((name, index) => [name, values[index].version === version
      ? sources.get(name) : `${JSON.stringify({ ...values[index], version }, null, 2)}\n`]),
  ].filter(([name, text]) => sources.get(name) !== text));
  checkVersionSnapshot(new Map([...sources, ...edits]));
  return edits;
}
