import assert from 'node:assert/strict';
import { readFileSync, existsSync, mkdirSync, mkdtempSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { prepareDriverPackage } from './prepare-driver-package.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const read = path => readFileSync(resolve(root, path), 'utf8');
const config = JSON.parse(read('apps/desktop/src-tauri/tauri.linux.conf.json'));

test('Linux package owns the device service and its user-session activation contract', () => {
  assert.deepEqual(config.bundle.externalBin, ['binaries/mixrelay-driver-service', 'binaries/mixrelay-service']);
  for (const format of ['deb', 'rpm']) {
    const files = config.bundle.linux[format].files;
    assert.ok(files['/usr/lib/systemd/user/mixrelay-driver.service']);
    assert.ok(files['/etc/xdg/autostart/mixrelay-driver.desktop']);
    assert.ok(files['/usr/share/mixrelay-driver/version.json']);
    for (const source of Object.values(files)) {
      if (source.includes('target/driver-package')) continue;
      assert.ok(existsSync(resolve(root, 'apps/desktop/src-tauri', source)), source);
    }
  }
  const unit = read('packaging/linux/mixrelay-driver.service');
  assert.match(unit, /^ExecStart=\/usr\/bin\/mixrelay-driver-service$/m);
  assert.doesNotMatch(unit, /User=root|WantedBy=multi-user|mixrelay-engine/);
  assert.match(read('packaging/linux/session-start.sh'), /systemctl --user start mixrelay-driver.service/);
  assert.ok(config.bundle.linux.deb.depends.includes('pkexec'));
  assert.ok(config.bundle.linux.rpm.depends.includes('polkit'));
  assert.ok(config.bundle.linux.rpm.depends.includes('libc.so.6(GLIBC_2.39)(64bit)'));
  assert.ok(!config.bundle.linux.rpm.depends.some(dependency => /\s[<>=]/.test(dependency)));
});

test('Driver preparation publishes actual matching service/version inputs without graph binaries', () => {
  const copies = [], writes = [], commands = [];
  const fsApi = {
    readFileSync(path) {
      return path.endsWith('Cargo.toml') ? '[workspace.package]\nversion = "1.2.3"\n' : '{"version":"1.2.3"}';
    },
    mkdirSync() {},
    copyFileSync(from, to) { copies.push([from, to]); },
    chmodSync() {},
    writeFileSync(path, text) { writes.push([path, text]); },
  };
  const result = prepareDriverPackage({
    args: ['--target', 'aarch64-unknown-linux-gnu'], rootDir: '/project', env: {}, fsApi,
    run(command, args) { commands.push([command, args]); return { status: 0 }; },
  });
  assert.equal(result.version, '1.2.3');
  assert.deepEqual(JSON.parse(writes[0][1]), { version: '1.2.3' });
  assert.match(copies[0][0], /release\/mixrelay-driver-service$/);
  assert.match(copies[0][1], /binaries\/mixrelay-driver-service-aarch64-unknown-linux-gnu$/);
  assert.ok(commands[0][1].includes('--locked'));
  assert.ok(commands[0][1].includes('mixrelay-driver-service'));
  assert.ok(!commands[0][1].includes('mixrelay-service'));
  fsApi.readFileSync = path => path.endsWith('Cargo.toml') ? '[workspace.package]\nversion = "1.2.4"\n' : '{"version":"1.2.3"}';
  assert.throws(() => prepareDriverPackage({ args: ['--target', 'aarch64-unknown-linux-gnu'], rootDir: '/project', env: {}, fsApi }), /versions must match/);
});

test('Driver preparation with --prebuilt uses existing slices and never runs cargo', () => {
  const commands = [];
  const present = new Set(['/project/target/aarch64-unknown-linux-gnu/release/mixrelay-driver-service']);
  const fsApi = {
    readFileSync(path) {
      return path.endsWith('Cargo.toml') ? '[workspace.package]\nversion = "1.2.3"\n' : '{"version":"1.2.3"}';
    },
    existsSync: path => present.has(path),
    mkdirSync() {},
    copyFileSync() {},
    chmodSync() {},
    writeFileSync() {},
  };
  const run = (command, args) => { commands.push([command, args]); return { status: 0 }; };
  prepareDriverPackage({ args: ['--prebuilt', '--target', 'aarch64-unknown-linux-gnu'], rootDir: '/project', env: {}, fsApi, run });
  assert.equal(commands.some(([command]) => command === 'cargo'), false);
  present.clear();
  assert.throws(
    () => prepareDriverPackage({ args: ['--target', 'aarch64-unknown-linux-gnu', '--prebuilt'], rootDir: '/project', env: {}, fsApi, run }),
    /Prebuilt Driver service is missing/,
  );
});

test('macOS Driver preparation with --prebuilt merges and signs both existing slices', () => {
  const release = slice => `/project/target/${slice}/release/mixrelay-driver-service`;
  const output = '/project/target/universal-apple-darwin/release/mixrelay-driver-service';
  const prepare = ({ present = [release('aarch64-apple-darwin'), release('x86_64-apple-darwin')], fail, archs = 'x86_64 arm64\n' } = {}) => {
    const commands = [], renames = [], removed = [];
    const fsApi = {
      readFileSync(path) {
        return path.endsWith('Cargo.toml') ? '[workspace.package]\nversion = "1.2.3"\n' : '{"version":"1.2.3"}';
      },
      existsSync: path => present.includes(path),
      mkdirSync() {},
      mkdtempSync: prefix => `${prefix}staging`,
      chmodSync() {},
      renameSync(from, to) { renames.push([from, to]); },
      rmSync(path) { removed.push(path); },
      writeFileSync() {},
    };
    const run = (command, args) => {
      commands.push([command, args]);
      if (args[0] === 'lipo' && args[1] === '-archs') return { status: fail === '-archs' ? 1 : 0, stdout: archs };
      return { status: command === fail || args[0] === fail ? 1 : 0 };
    };
    const call = () => prepareDriverPackage({
      args: ['--profile', 'release', '--target', 'aarch64-apple-darwin', '--prebuilt'],
      rootDir: '/project', env: { MIXRELAY_CODESIGN_IDENTITY: 'Developer ID Application: MixRelay (AB12C3D4E5)' }, fsApi, run,
    });
    return { call, commands, renames, removed };
  };

  const success = prepare();
  assert.equal(success.call().output, output);
  assert.equal(success.commands.some(([command]) => command === 'cargo'), false);
  assert.deepEqual(success.commands[0], ['xcrun', ['lipo', '-create', release('aarch64-apple-darwin'), release('x86_64-apple-darwin'), '-output', '/project/target/universal-apple-darwin/release/.driver-service-staging/mixrelay-driver-service']]);
  assert.deepEqual(success.commands.map(([command, args]) => `${command} ${args.slice(0, 2).join(' ')}`.trim()),
    ['xcrun lipo -create', 'xcrun lipo -archs', `codesign --force --sign`, 'codesign --verify --strict']);
  assert.deepEqual(success.commands[1][1], ['lipo', '-archs', '/project/target/universal-apple-darwin/release/.driver-service-staging/mixrelay-driver-service']);
  assert.deepEqual(success.renames.map(([, to]) => to), [output]);
  assert.deepEqual(success.removed, ['/project/target/universal-apple-darwin/release/.driver-service-staging']);

  for (const slice of ['aarch64-apple-darwin', 'x86_64-apple-darwin']) {
    const missing = prepare({ present: [release(slice === 'aarch64-apple-darwin' ? 'x86_64-apple-darwin' : 'aarch64-apple-darwin')] });
    assert.throws(missing.call, new RegExp(`Prebuilt Driver service is missing: .*${slice}`));
    assert.deepEqual(missing.commands, []);
    assert.deepEqual(missing.renames, []);
  }
  // A failed merge or signature never replaces the previous output and always removes staging.
  for (const fail of ['lipo', '--force', '-archs']) {
    const failed = prepare({ fail });
    assert.throws(failed.call, /Driver build failed/);
    assert.deepEqual(failed.renames, []);
    assert.equal(failed.removed.length, 1);
  }
  // A merge missing, duplicating or adding a slice is never published or signed.
  for (const archs of ['arm64\n', 'x86_64\n', 'arm64 x86_64 arm64e\n', 'arm64 arm64\n', '']) {
    const wrong = prepare({ archs });
    assert.throws(wrong.call, /must contain exactly arm64 and x86_64/, JSON.stringify(archs));
    assert.deepEqual(wrong.renames, []);
    assert.equal(wrong.commands.some(([command]) => command === 'codesign'), false);
    assert.equal(wrong.removed.length, 1);
  }
});

test('Linux service packaging makes a restrictive build output executable for ordinary users', t => {
  const directory = mkdtempSync(join(root, 'target/driver-sidecar-permissions-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  mkdirSync(join(directory, 'apps/desktop/src-tauri'), { recursive: true });
  writeFileSync(join(directory, 'Cargo.toml'), '[workspace.package]\nversion = "1.2.3"\n');
  writeFileSync(join(directory, 'apps/desktop/src-tauri/tauri.conf.json'), '{"version":"1.2.3"}');
  const input = join(directory, 'target/x86_64-unknown-linux-gnu/release/mixrelay-driver-service');
  const bytes = '#!/bin/sh\nexit 0\n';
  mkdirSync(dirname(input), { recursive: true });
  writeFileSync(input, bytes, { mode: 0o700 });
  prepareDriverPackage({ args: ['--target', 'x86_64-unknown-linux-gnu'], rootDir: directory,
    env: {}, run: () => ({ status: 0 }) });
  const sidecar = join(directory, 'apps/desktop/src-tauri/binaries/mixrelay-driver-service-x86_64-unknown-linux-gnu');
  assert.equal(readFileSync(sidecar, 'utf8'), bytes);
  assert.equal(statSync(sidecar).mode & 0o777, 0o755);
  assert.equal(statSync(input).mode & 0o777, 0o700);
});

test('macOS device service ownership, autostart and optional cleanup preserve App boundaries', () => {
  const contract = read('native/macos/installer/src/contract.rs');
  assert.match(contract, /include!\(concat!\(env!\("OUT_DIR"\), "\/product_paths\.rs"\)\)/);
  assert.match(contract, /bin\/mixrelay-service/);
  const launchd = read('packaging/macos/io.mixrelay.audio.driver.plist.in');
  assert.match(launchd, /<key>KeepAlive<\/key><true\/>/);
  assert.match(launchd, /<key>RunAtLoad<\/key><true\/>/);
  assert.match(launchd, /<key>UserName<\/key><string>__MIXRELAY_SERVICE_USER__/);
  const cleanup = read('native/macos/installer/src/native/user_data.rs').split('#[cfg(test)]')[0];
  assert.match(cleanup, /if !delete_settings \{\s*return Ok\(\(\)\);\s*\}[\s\S]*remove_file\(&parent, c"drivers\.json"\)/);
  assert.doesNotMatch(cleanup, /mixrelay_licensing|graphs\.json|recording-settings\.json/);
});
