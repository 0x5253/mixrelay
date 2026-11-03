import assert from 'node:assert/strict';
import test from 'node:test';
import { checkVersion, productCrates, productVersion, setVersion, workspaceMembers } from './product-version.mjs';
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';

test('all release entry points share the Cargo product version', () => {
  assert.equal(checkVersion(), productVersion());
  const desktop = JSON.parse(readFileSync(new URL('../apps/desktop/src-tauri/tauri.conf.json', import.meta.url)));
  assert.equal(desktop.identifier, 'io.mixrelay.desktop');
  assert.equal(desktop.productName, 'MixRelay');
  const linux = JSON.parse(readFileSync(new URL('../apps/desktop/src-tauri/tauri.linux.conf.json', import.meta.url)));
  assert.deepEqual(linux.bundle.externalBin, ['binaries/mixrelay-driver-service', 'binaries/mixrelay-service']);
  for (const format of ['deb', 'rpm']) {
    const config = linux.bundle.linux[format];
    assert.equal(config.recommends, undefined);
    assert.ok(config.files['/usr/lib/systemd/user/mixrelay-driver.service']);
  }
});

function versionFixture(t) {
  const parent = new URL('../target/test-artifacts/', import.meta.url);
  mkdirSync(parent, { recursive: true });
  const directory = mkdtempSync(new URL('product-version-', parent));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const paths = ['Cargo.toml', 'Cargo.lock', 'package.json', 'apps/control-ui/package.json', 'apps/desktop/package.json', 'apps/desktop/src-tauri/tauri.conf.json', 'native/macos/hal-driver/driver/Info.plist',
    ...workspaceMembers().map(member => `${member}/Cargo.toml`)];
  for (const path of paths) {
    mkdirSync(dirname(join(directory, path)), { recursive: true });
    copyFileSync(new URL(`../${path}`, import.meta.url), join(directory, path));
  }
  return { directory, paths };
}
const lockedVersion = (lock, name) => lock.match(new RegExp(`^name = "${name}"\\nversion = "([^"]+)"`, 'm'))?.[1];

test('one version command updates app, HAL and locked workspace crate metadata together', t => {
  const { directory } = versionFixture(t);
  const lockBefore = readFileSync(join(directory, 'Cargo.lock'), 'utf8');
  assert.equal(setVersion('0.2.0', directory), '0.2.0');
  assert.equal(checkVersion(directory), '0.2.0');
  const hal = readFileSync(join(directory, 'native/macos/hal-driver/driver/Info.plist'), 'utf8');
  assert.match(hal, /<key>CFBundleShortVersionString<\/key>\s*<string>0\.2\.0<\/string>/);
  const lockAfter = readFileSync(join(directory, 'Cargo.lock'), 'utf8');
  const before = lockBefore.split('\n');
  const after = lockAfter.split('\n');
  assert.equal(after.length, before.length);
  const crates = productCrates(directory);
  const changed = before.flatMap((line, index) => line === after[index] ? [] : [after[index - 1].match(/^name = "([^"]+)"$/)?.[1]]);
  assert.deepEqual(changed.toSorted(), crates.toSorted(), 'only the version line of each crate that inherits the product version changes');
  for (const name of crates) assert.equal(lockedVersion(lockAfter, name), '0.2.0');
  // A fixture with its own version, vendored crates and a registry crate at the old number keep theirs.
  assert.equal(lockedVersion(lockAfter, 'mixrelay-native-clap-fixture'), '0.1.0');
  assert.equal(lockedVersion(lockAfter, 'truce-rack-core'), '1.1.5');
  assert.match(lockAfter, /^name = "jiff-core"\nversion = "0\.1\.0"\nsource = "registry\+/m);
  assert.equal(setVersion('0.1.0', directory), '0.1.0');
  assert.equal(readFileSync(join(directory, 'Cargo.lock'), 'utf8'), lockBefore);
});

test('setting the current version changes no bytes', t => {
  const { directory, paths } = versionFixture(t);
  const before = paths.map(path => readFileSync(join(directory, path)));
  setVersion(productVersion(directory), directory);
  paths.forEach((path, index) => assert.deepEqual(readFileSync(join(directory, path)), before[index], path));
});

test('a stale locked workspace crate fails the version check', t => {
  const { directory } = versionFixture(t);
  setVersion('0.2.0', directory);
  const lockPath = join(directory, 'Cargo.lock');
  writeFileSync(lockPath, readFileSync(lockPath, 'utf8').replace(/^(name = "mixrelay-desktop"\nversion = ")0\.2\.0/m, '$10.1.0'));
  assert.throws(() => checkVersion(directory), /Cargo\.lock mixrelay-desktop differs from MixRelay 0\.2\.0/);
});

test('invalid HAL metadata rejects a version change before writing any manifest', t => {
  const { directory, paths } = versionFixture(t);
  writeFileSync(join(directory, 'native/macos/hal-driver/driver/Info.plist'), '<plist/>');
  const before = paths.map(path => readFileSync(join(directory, path)));
  assert.throws(() => setVersion('0.2.0', directory), /missing CFBundle/);
  paths.forEach((path, index) => assert.deepEqual(readFileSync(join(directory, path)), before[index]));
});

test('a lockfile without a workspace crate rejects a version change before writing any manifest', t => {
  const { directory, paths } = versionFixture(t);
  const lockPath = join(directory, 'Cargo.lock');
  writeFileSync(lockPath, readFileSync(lockPath, 'utf8').replace(/^name = "mixrelay-desktop"$/m, 'name = "renamed-desktop"'));
  const before = paths.map(path => readFileSync(join(directory, path)));
  assert.throws(() => setVersion('0.2.0', directory), /Cargo\.lock is missing workspace crates: mixrelay-desktop$/);
  paths.forEach((path, index) => assert.deepEqual(readFileSync(join(directory, path)), before[index]));
});
