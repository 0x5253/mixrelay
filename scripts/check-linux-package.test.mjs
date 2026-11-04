import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import test from 'node:test';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { join } from 'node:path';
import { PassThrough, Writable } from 'node:stream';
import { inspectPackage, validatePackage } from './check-linux-package.mjs';

const releaseDocuments = ["LICENSE.txt"];
const wirePlumberFiles = [
  'usr/share/wireplumber/scripts/mixrelay-source-mute.lua',
  'usr/share/wireplumber/wireplumber.conf.d/90-mixrelay-source-mute.conf',
];
const uninstallFiles = [
  'usr/lib/mixrelay/uninstall.sh',
  'usr/share/applications/mixrelay-uninstall.desktop',
];
const receiptPath = 'usr/share/mixrelay-driver/version.json';
const documentationFiles = [
  ...releaseDocuments.map(filename => `usr/share/doc/mix-relay/${filename}`),
  'usr/lib/systemd/user/mixrelay-driver.service',
  'etc/xdg/autostart/mixrelay-driver.desktop',
  'usr/lib/mixrelay-driver/session-start.sh',
  'usr/lib/mixrelay-driver/before-change.sh',
  receiptPath,
  ...releaseDocuments.map(filename => `usr/share/doc/mix-relay-driver/${filename}`),
  'usr/share/doc/mix-relay-driver/third-party/truce-rack/LICENSE-MIT',
  'usr/share/doc/mix-relay-driver/third-party/truce-rack/LICENSE-APACHE',
];
const binaries = ['mixrelay-desktop', 'mixrelay-service', 'mixrelay-driver-service'];
const lifecycleHooks = Object.fromEntries(Object.entries({
  preinst: 'driver-before-change.sh', prerm: 'driver-pre-remove.sh',
  postinst: 'driver-after-install.sh', postrm: 'driver-after-remove.sh',
}).map(([script, source]) => [script, readFileSync(new URL(`../packaging/linux/${source}`, import.meta.url), 'utf8')]));
const linuxConfig = () => JSON.parse(readFileSync(new URL('../apps/desktop-shell/src-tauri/tauri.linux.conf.json', import.meta.url), 'utf8'));

function metadataOutput(name, data) {
  if (name === 'Package') return data.packageName;
  if (name === 'Version') return data.version;
  if (name === 'Homepage') return data.homepage;
  if (name === 'Maintainer') return data.maintainer;
  if (Object.hasOwn(lifecycleHooks, name)) return lifecycleHooks[name];
  return name === 'Depends' ? data.depends : data.architecture;
}

function elfHeader(machine) {
  const header = Buffer.alloc(64);
  header.set([0x7f, 0x45, 0x4c, 0x46, 2, 1, 1]);
  header[16] = 2;
  header[18] = machine & 0xff;
  header[19] = machine >> 8;
  header[20] = 1;
  header[52] = 64;
  return header;
}

const fixture = (architecture = 'arm64') => ({
  packageName: 'mix-relay',
  homepage: 'https://rawsector.com/products/mixrelay',
  maintainer: 'rawsector <support@rawsector.com>',
  version: '0.1.0',
  versionReceipt: '{"version":"0.1.0"}'.padEnd(64),
  architecture: `${architecture}\n`,
  depends: 'libpipewire-0.3-0t64 (>= 1.2.0), liblilv-0-0, libgcc-s1, libc6 (>= 2.39), libwebkit2gtk-4.1-0, libgtk-3-0t64, librsvg2-common, pipewire (>= 1.2.0), wireplumber (>= 0.5.0), libayatana-appindicator3-1, zenity, pkexec, polkit-1-auth-agent | lxpolkit, systemd, util-linux',
  contents: [
    ...binaries.map(name => `-rwxr-xr-x 0/0 64 2026-09-20 22:00 ./usr/bin/${name}`),
    ...wirePlumberFiles.map(path => `-rw-r--r-- 0/0 64 2026-09-20 22:00 ./${path}`),
    ...uninstallFiles.map(path => `-rw-r--r-- 0/0 64 2026-09-20 22:00 ./${path}`),
    ...documentationFiles.map(path => `-rw-r--r-- 0/0 64 2026-09-20 22:00 ./${path}`),
  ].join('\n'),
  elfHeaders: Object.fromEntries(binaries.map(name => [`usr/bin/${name}`, elfHeader(architecture === 'arm64' ? 183 : 62)])),
});
const payloads = data => new Map([...Object.entries(data.elfHeaders), [receiptPath, Buffer.from(data.versionReceipt)]]);

function fakeExtractor(payloadsByPath, calls = [], numericListing, kills = [], rawChunks = [Buffer.from('bounded fixture tar bytes')]) {
  return (command, args, options) => {
    calls.push([command, args, options]);
    const child = new EventEmitter();
    child.stdout = new PassThrough();
    child.stderr = new PassThrough();
    child.kill = signal => { kills.push([command, signal]); return true; };
    if (command === 'dpkg-deb') {
      process.nextTick(() => {
        child.stderr.end();
        for (const chunk of rawChunks) child.stdout.write(chunk);
        child.stdout.end();
        child.stdout.on('end', () => setImmediate(() => child.emit('close', 0, null)));
      });
      return child;
    }
    assert.equal(command, 'tar');
    child.stdin = new Writable({ write(_chunk, _encoding, callback) { callback(); } });
    child.stdin.on('finish', () => {
      process.nextTick(() => {
        child.stderr.end();
        if (args[0] === '--numeric-owner') {
          child.stdout.end(numericListing ?? '');
        } else {
          const memberPath = args.at(-1).replace(/^(?:\.\/)+/, '');
          child.stdout.end(payloadsByPath.get(memberPath) ?? Buffer.alloc(0));
        }
        child.stdout.on('end', () => setImmediate(() => child.emit('close', 0, null)));
      });
    });
    return child;
  };
}

test('Debian ARM64 and amd64 payloads include every executable owner and dependency', () => {
  for (const architecture of ['arm64', 'amd64']) {
    const result = validatePackage(fixture(architecture));
    assert.equal(result.packageName, 'mix-relay');
    assert.equal(result.architecture, architecture);
    assert.deepEqual(result.elfArchitectures, Object.fromEntries(binaries.map(name =>
      [`usr/bin/${name}`, architecture === 'arm64' ? 'AArch64' : 'x86-64'])));
  }
});
test('both Linux package formats own the WirePlumber hook and policy fragment', () => {
  const config = linuxConfig();
  for (const format of ['deb', 'rpm']) {
    for (const path of wirePlumberFiles) {
      assert.ok(config.bundle.linux[format].files[`/${path}`]);
    }
  }
});
test('shared resources and both Linux packages include unchanged licence notices', () => {
  const shared = JSON.parse(readFileSync(new URL(
    '../apps/desktop-shell/src-tauri/tauri.conf.json', import.meta.url,
  ), 'utf8'));
  const linux = JSON.parse(readFileSync(new URL(
    '../apps/desktop-shell/src-tauri/tauri.linux.conf.json', import.meta.url,
  ), 'utf8'));
  for (const name of releaseDocuments) {
    assert.equal(shared.bundle.resources[`../../../${name}`], name);
    assert.ok(readFileSync(new URL(`../${name}`, import.meta.url), 'utf8').trim());
    for (const format of ['deb', 'rpm']) {
      assert.equal(linux.bundle.linux[format].files[`/usr/share/doc/mix-relay/${name}`],
        `../../../${name}`);
    }
  }
  for (const license of ['LICENSE-MIT', 'LICENSE-APACHE']) {
    assert.equal(shared.bundle.resources[`../../../vendor/${license}`],
      `third-party/truce-rack/${license}`);
  }
});
test('package documentation cannot be missing, empty, linked, or unreadable to installed users', () => {
  const data = fixture();
  const invalidate = [
    () => null,
    line => line.replace(' 64 ', ' 0 '),
    line => line.replace('-rw-r--r--', 'lrwxrwxrwx'),
    line => line.replace('-rw-r--r--', 'drwxr-xr-x'),
    line => line.replace('-rw-r--r--', '-rw-------'),
    line => line.replace('0/0', '1000/1000'),
  ];
  for (const path of documentationFiles) {
    for (const mutateLine of invalidate) {
      const contents = data.contents.split('\n').map(line =>
        line.endsWith(`./${path}`) ? mutateLine(line) : line).filter(line => line !== null).join('\n');
      assert.throws(() => validatePackage({ ...data, contents }), error =>
        error.message === `Missing or invalid package documentation: ${path}`);
    }
  }
});
test('Linux product builds select package-manager formats and register lifecycle hooks', () => {
  const config = JSON.parse(readFileSync(new URL(
    '../apps/desktop-shell/src-tauri/tauri.linux.conf.json', import.meta.url,
  ), 'utf8'));
  assert.deepEqual(config.bundle.targets, ['deb', 'rpm']);
  for (const format of ['deb', 'rpm']) {
    assert.equal(config.bundle.linux[format].preInstallScript,
      '../../../packaging/linux/driver-before-change.sh');
  }
  assert.equal(config.bundle.linux.deb.preRemoveScript,
    '../../../packaging/linux/driver-pre-remove.sh');
  assert.equal(config.bundle.linux.rpm.preRemoveScript,
    '../../../packaging/linux/driver-rpm-pre-remove.sh');
  for (const name of ['gtk3', 'webkit2gtk4.1', 'librsvg2']) {
    assert.ok(config.bundle.linux.rpm.depends.includes(name));
  }
  assert.ok(config.bundle.linux.deb.depends.includes('pkexec'));
  assert.ok(config.bundle.linux.deb.depends.includes('polkit-1-auth-agent | lxpolkit'));
  assert.ok(config.bundle.linux.rpm.depends.includes('polkit'));
  assert.ok(config.bundle.linux.rpm.depends.includes('PolicyKit-authentication-agent'));
  for (const format of ['deb', 'rpm']) {
    for (const path of uninstallFiles) assert.ok(config.bundle.linux[format].files[`/${path}`]);
  }
});
test('package replacement drains running installed apps before the Driver and preserves user files', t => {
  mkdirSync(new URL('../target/', import.meta.url), { recursive: true });
  const directory = mkdtempSync(new URL('../target/linux-package-hook-', import.meta.url));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const proc = join(directory, 'proc'), calls = join(directory, 'calls');
  for (const pid of ['123', '124', '125']) mkdirSync(join(proc, pid), { recursive: true });
  mkdirSync(join(directory, 'runtime'));
  const userFile = join(directory, 'saved-graph.json');
  writeFileSync(userFile, '{"preserved":true}');
  const commands = {
    stat: 'printf "%s\\n" "${FIXTURE_UID:-1000}"',
    getent: 'printf "qa:x:1000:1000:QA:/home/qa:/bin/sh\\n"',
    sleep: ':',
    signal: `printf '%s %s\\n' "$1" "$2" >> '${calls}'
[ "$1" = -TERM ] || exit 99
[ "$FIXTURE_STUCK" != 1 ] || exit 0
rm -f '${proc}/123/exe' '${proc}/124/exe'`,
    runuser: `[ ! -L '${proc}/123/exe' ] && [ ! -L '${proc}/124/exe' ] || exit 98
printf 'driver-stop\\n' >> '${calls}'
rm -f '${proc}/125/exe'`,
  };
  for (const [name, source] of Object.entries(commands))
    writeFileSync(join(directory, name), `#!/bin/sh\n${source}\n`, { mode: 0o755 });
  const runHook = (name, args, environment = {}) => {
    const source = readFileSync(new URL(`../packaging/linux/${name}`, import.meta.url), 'utf8');
    const fixtureHook = join(directory, name);
    writeFileSync(fixtureHook, source.replaceAll('/proc/[0-9]*/exe', `${proc}/[0-9]*/exe`)
      .replace('/usr/lib/mixrelay-driver/before-change.sh', join(directory, 'driver-before-change.sh'))
      .replace('/bin/kill', join(directory, 'signal')).replace('/run/user/$uid', join(directory, 'runtime')));
    return spawnSync('sh', [fixtureHook, ...args], { encoding: 'utf8',
      env: { ...process.env, PATH: `${directory}:${process.env.PATH}`, LC_ALL: 'C', ...environment } });
  };
  for (const deleted of ['', ' (deleted)']) {
    symlinkSync(`/usr/bin/mixrelay-desktop${deleted}`, join(proc, '123/exe'));
    symlinkSync(`/usr/bin/mixrelay-service${deleted}`, join(proc, '124/exe'));
    symlinkSync(`/usr/bin/mixrelay-driver-service${deleted}`, join(proc, '125/exe'));
    writeFileSync(calls, '');
    const result = runHook('driver-before-change.sh', ['upgrade']);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(readFileSync(calls, 'utf8'), '-TERM 123\ndriver-stop\n');
  }
  symlinkSync('/usr/bin/mixrelay-desktop', join(proc, '123/exe'));
  assert.equal(runHook('driver-before-change.sh', ['upgrade'], { FIXTURE_UID: '0' }).status, 1);
  assert.equal(runHook('driver-pre-remove.sh', ['upgrade']).status, 0);
  assert.equal(runHook('driver-rpm-pre-remove.sh', ['1']).status, 0);
  for (const hook of ['driver-before-change.sh', 'driver-pre-remove.sh', 'driver-rpm-pre-remove.sh']) {
    const result = runHook(hook, [hook.includes('rpm') ? '0' : 'remove'], { FIXTURE_STUCK: '1' });
    assert.equal(result.status, 1, result.stderr);
    assert.match(result.stderr, /Stop all audio.*close the MixRelay editor/);
    assert.ok(readFileSync(calls, 'utf8').split('\n').every(line => !line.includes('KILL')));
  }
  rmSync(join(proc, '123/exe'));
  symlinkSync('/home/test/development/mixrelay-service', join(proc, '124/exe'));
  assert.equal(runHook('driver-before-change.sh', ['upgrade']).status, 0);
  assert.equal(readFileSync(userFile, 'utf8'), '{"preserved":true}');
});
test('graphical uninstall deletes only selected app data after successful package removal', t => {
  mkdirSync(new URL('../target/', import.meta.url), { recursive: true });
  const directory = mkdtempSync(new URL('../target/linux-uninstall-', import.meta.url));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const source = readFileSync(new URL('../packaging/linux/uninstall.sh', import.meta.url), 'utf8');
  const scenarios = ['preserve', 'delete', 'failure', 'cancel', 'auth-cancel',
    'root', 'symlink', 'unsafe-root', 'rpm', 'arch', 'relative-xdg', 'korean'];
  for (const scenario of scenarios) {
    const fixtureDirectory = join(directory, scenario);
    const userDirectory = join(fixtureDirectory, 'home');
    const config = join(userDirectory, '.config/mixrelay');
    const data = join(userDirectory, '.local/share/io.mixrelay.desktop');
    mkdirSync(join(config, 'graphs.states'), { recursive: true });
    mkdirSync(join(data, 'WebKitCache'), { recursive: true });
    const deletedPaths = [join(config, 'graphs.json'), join(config, 'audio-session.json'),
      join(config, 'graphs.states/state.blob'), join(data, 'recording-settings.json'),
      join(data, 'WebKitCache/cache.blob')];
    const preservedPaths = [join(userDirectory, 'recording.wav'),
      join(userDirectory, 'exported-project.json'), join(config, 'unrelated.txt')];
    for (const path of [...deletedPaths, ...preservedPaths]) {
      writeFileSync(path, 'preserve unless selected');
    }
    if (scenario === 'symlink') {
      const originalConfig = join(userDirectory, '.config');
      // A symlink at any parent is rejected, even when its target happens to
      // contain the expected application filename.
      symlinkSync(originalConfig, join(userDirectory, 'linked-config'));
    }
    const log = join(fixtureDirectory, 'calls');
    const commands = {
      id: 'printf "%s\\n" "$TEST_UID"',
      'dpkg-query': '[ "$TEST_PACKAGE" = deb ] || exit 1\nprintf installed',
      rpm: '[ "$TEST_PACKAGE" = rpm ]',
      pacman: '[ "$TEST_PACKAGE" = arch ]',
      apt: 'exit 99',
      dnf: 'exit 99',
      zenity: `printf '%s\\n' "$@" >> "$TEST_LOG"
if [ "$1" = --list ]; then
  [ "$TEST_CANCEL" != 1 ] || exit 1
  for value do last=$value; done
  [ "$TEST_SELECT" != 1 ] || printf '%s\\n' "$last"
fi`,
      pkexec: `printf '%s\\n' "$@" >> "$TEST_LOG"
[ "$TEST_FAIL" != 1 ] || exit 1
[ "$TEST_CANCEL_AUTH" != 1 ] || exit 126
rm -f -- "$TEST_SCRIPT" "$TEST_DESKTOP"`,
    };
    let fixtureSource = source;
    for (const [command, body] of Object.entries(commands)) {
      const commandPath = join(fixtureDirectory, command);
      writeFileSync(commandPath, `#!/bin/sh\n${body}\n`, { mode: 0o755 });
      fixtureSource = fixtureSource.replaceAll(`/usr/bin/${command}`, commandPath);
    }
    // Linux installs rm in /usr/bin; macOS keeps the host's fixture cleanup
    // tool in /bin. Only the test copy adapts that fixed executable path.
    if (process.platform === 'darwin') {
      fixtureSource = fixtureSource.replaceAll('/usr/bin/rm', '/bin/rm');
    }
    const script = join(fixtureDirectory, 'uninstall.sh');
    writeFileSync(script, fixtureSource);
    const result = spawnSync('sh', [script], { encoding: 'utf8', env: {
      ...process.env,
      HOME: userDirectory,
      XDG_CONFIG_HOME: scenario === 'symlink' ? join(userDirectory, 'linked-config')
        : scenario === 'relative-xdg' ? 'relative-config' : '',
      XDG_DATA_HOME: scenario === 'unsafe-root' ? '/'
        : scenario === 'relative-xdg' ? 'relative-data' : '',
      LC_ALL: scenario === 'korean' ? '' : 'C',
      LC_MESSAGES: '',
      LANG: scenario === 'korean' ? 'ko_KR.UTF-8' : 'C',
      TEST_LOG: log,
      TEST_SCRIPT: script,
      TEST_DESKTOP: join(fixtureDirectory, 'mixrelay-desktop'),
      TEST_UID: scenario === 'root' ? '0' : '1000',
      TEST_PACKAGE: ['rpm', 'arch'].includes(scenario) ? scenario : 'deb',
      TEST_SELECT: scenario === 'preserve' ? '0' : '1',
      TEST_CANCEL: scenario === 'cancel' ? '1' : '0',
      TEST_FAIL: scenario === 'failure' ? '1' : '0',
      TEST_CANCEL_AUTH: scenario === 'auth-cancel' ? '1' : '0',
    } });
    const deleting = ['delete', 'rpm', 'arch', 'relative-xdg', 'korean'].includes(scenario);
    const successful = scenario === 'preserve' || deleting;
    const removed = successful;
    assert.equal(result.status, successful || scenario === 'cancel' ? 0 : 1, result.stderr);
    for (const path of preservedPaths) assert.ok(existsSync(path), `${scenario} preserved ${path}`);
    for (const path of deletedPaths) {
      assert.equal(existsSync(path), !deleting, `${scenario}: ${path}`);
    }
    const calls = readFileSync(log, 'utf8');
    if (scenario !== 'root') assert.match(calls, scenario === 'korean'
      ? /FALSE\n저장된 그래프와 설정 삭제/ : /FALSE\nDelete saved graphs and settings/);
    if (removed || scenario === 'failure' || scenario === 'auth-cancel') {
      assert.match(calls, /--disable-internal-agent/);
      assert.match(calls, scenario === 'arch' ? /-R\n--noconfirm\nmix-relay/ : /-y\nremove\nmix-relay/);
      assert.ok(calls.includes(join(fixtureDirectory, { rpm: 'dnf', arch: 'pacman' }[scenario] ?? 'apt')));
      if (scenario === 'rpm') assert.match(calls, /clean_requirements_on_remove=False/);
    } else assert.doesNotMatch(calls, /--disable-internal-agent/);
    if (removed) {
      assert.ok(!existsSync(script), 'cleanup continues after package deletes script');
      assert.ok(!existsSync(join(fixtureDirectory, 'mixrelay-desktop')));
    }
  }
});
test('Debian package requires its graphical uninstaller and privilege authorization dependency', () => {
  const data = fixture();
  for (const path of uninstallFiles) {
    assert.throws(() => validatePackage({ ...data,
      contents: data.contents.split('\n').filter(line => !line.endsWith(`./${path}`)).join('\n'),
    }), /Missing or invalid uninstall payload/);
  }
  assert.throws(() => validatePackage({ ...data,
    depends: data.depends.split(',').filter(term => term.trim() !== 'pkexec').join(','),
  }), /Missing required dependency: pkexec/);
  assert.throws(() => validatePackage({ ...data,
    depends: data.depends.split(',')
      .filter(term => term.trim() !== 'polkit-1-auth-agent | lxpolkit').join(','),
  }), /Missing required dependency: polkit-1-auth-agent or lxpolkit/);
});
test('WirePlumber payloads must be non-empty regular root-owned readable data files', () => {
  const data = fixture();
  for (const path of wirePlumberFiles) {
    const lines = data.contents.split('\n');
    const index = lines.findIndex(line => line.endsWith(`./${path}`));
    assert.notEqual(index, -1);
    const mutate = change => lines.map((line, position) => position === index ? change(line) : line).join('\n');
    assert.throws(() => validatePackage({ ...data, contents: lines.filter((_, position) => position !== index).join('\n') }),
      new RegExp(`Missing non-empty regular WirePlumber payload: ${path}`));
    for (const change of [
      line => line.replace(' 64 ', ' 0 '),
      line => line.replace('-rw-r--r--', 'lrwxrwxrwx'),
      line => line.replace('-rw-r--r--', 'drwxr-xr-x'),
    ]) {
      assert.throws(() => validatePackage({ ...data, contents: mutate(change) }),
        new RegExp(`Missing non-empty regular WirePlumber payload: ${path}`));
    }
    for (const change of [
      line => line.replace('0/0', '1000/1000'),
      line => line.replace('0/0', 'root/root'),
      line => line.replace('-rw-r--r--', '-rw-r-----'),
      line => line.replace('-rw-r--r--', '-rw-rw-r--'),
      line => line.replace('-rw-r--r--', '-rwxr-xr-x'),
    ]) {
      assert.throws(() => validatePackage({ ...data, contents: mutate(change) }),
        new RegExp(`Invalid WirePlumber payload ownership or permissions: ${path}`));
    }
  }
});
test('required dependencies cannot be optional alternatives', () => {
  const data = fixture();
  data.depends = data.depends.replace('liblilv-0-0', 'liblilv-0-0 | unrelated');
  assert.throws(() => validatePackage(data), /liblilv/);
});
test('audio session, native dialogs and user-service tools are mandatory in Debian and RPM', () => {
  const config = linuxConfig();
  const names = ['pipewire', 'wireplumber', 'zenity', 'systemd', 'util-linux'];
  for (const format of ['deb', 'rpm']) {
    for (const name of names) {
      assert.ok(config.bundle.linux[format].depends.some(term => term.split(/\s/)[0] === name));
    }
  }
  for (const name of names) {
    const data = fixture();
    data.depends = data.depends.split(',').filter(term => term.trim().split(/\s/)[0] !== name).join(',');
    assert.throws(() => validatePackage(data), new RegExp(`Missing required dependency: ${name}`));
  }
});
test('Debian SVG icon loader is a required package dependency', () => {
  const config = JSON.parse(readFileSync(new URL(
    '../apps/desktop-shell/src-tauri/tauri.linux.conf.json', import.meta.url,
  ), 'utf8'));
  assert.ok(config.bundle.linux.deb.depends.includes('librsvg2-common'));
  const data = fixture();
  data.depends = data.depends.replace(', librsvg2-common', '');
  assert.throws(() => validatePackage(data), /Missing required dependency: librsvg2-common/);
});
test('tray indicator runtime provider is mandatory in Debian payloads and both package configurations', () => {
  const config = JSON.parse(readFileSync(new URL(
    '../apps/desktop-shell/src-tauri/tauri.linux.conf.json', import.meta.url,
  ), 'utf8'));
  assert.ok(config.bundle.linux.deb.depends.includes('libayatana-appindicator3-1'));
  assert.ok(config.bundle.linux.rpm.depends.includes('libayatana-appindicator-gtk3'));

  const data = fixture();
  data.depends = data.depends.split(',')
    .filter(term => !['libayatana-appindicator3-1', 'libappindicator3-1'].includes(term.trim()))
    .join(',');
  assert.throws(() => validatePackage(data), /Missing required dependency: libayatana-appindicator3-1 or libappindicator3-1/);

  const alternativeProvider = fixture();
  alternativeProvider.depends = alternativeProvider.depends.replace('libayatana-appindicator3-1', 'libappindicator3-1');
  assert.doesNotThrow(() => validatePackage(alternativeProvider));
});
test('unsupported dependency qualifiers and restrictions fail closed', () => {
  for (const dependency of ['libc6:amd64', 'libc6 [!arm64]', 'libc6 <stage1>']) {
    const data = fixture();
    data.depends = data.depends.replace('libc6 (>= 2.39)', dependency);
    assert.throws(() => validatePackage(data), /Unsupported dependency qualifier or restriction/);
  }
});
test('missing, symlink, non-executable and duplicate engines fail', () => {
  const data = fixture();
  for (const contents of [data.contents.split('\n')[0], data.contents.replaceAll('-rwx', 'lrwx'),
    data.contents.replaceAll('-rwxr-xr-x', '-rw-r--r--'), `${data.contents}\n${data.contents}`]) {
    assert.throws(() => validatePackage({ ...data, contents }));
  }
});
test('desktop and service executables require root ownership and exact 0755 permissions', () => {
  const data = fixture();
  const invalidate = [
    line => line.replace('0/0', '1000/1000'),
    line => line.replace('0/0', 'root/root'),
    line => line.replace('0/0', '1000/0'),
    line => line.replace('0/0', '0/1000'),
    line => line.replace('-rwxr-xr-x', '-rwxrwxr-x'),
    line => line.replace('-rwxr-xr-x', '-rwxr-xrwx'),
    line => line.replace('-rwxr-xr-x', '-rwsr-xr-x'),
  ];
  for (const executableName of binaries) {
    for (const mutateLine of invalidate) {
      const contents = data.contents.split('\n').map(line =>
        line.endsWith(`/${executableName}`) ? mutateLine(line) : line).join('\n');
      assert.throws(() => validatePackage({ ...data, contents }), /ownership or permissions/);
    }
  }
});
test('zero-length required executables fail', () => {
  const data = fixture();
  assert.throws(() => validatePackage({ ...data, contents: data.contents.replace(/0\/0 \d+ /g, '0/0 0 ') }),
    /non-empty regular executable/);
});
test('ELF machine, class, byte order, and header validity must match package architecture', () => {
  const data = fixture('arm64');
  for (const path of binaries.map(name => `usr/bin/${name}`)) {
    assert.throws(() => validatePackage({
      ...data,
      elfHeaders: { ...data.elfHeaders, [path]: elfHeader(62) },
    }), new RegExp(`Unexpected ELF architecture for ${path}`));
  }
  for (const invalidHeader of [
    Buffer.from(elfHeader(183)).fill(0, 0, 4),
    Buffer.from(elfHeader(183)).fill(1, 4, 5),
    Buffer.from(elfHeader(183)).fill(2, 5, 6),
    Buffer.alloc(32),
  ]) {
    assert.throws(() => validatePackage({
      ...data,
      elfHeaders: { ...data.elfHeaders, 'usr/bin/mixrelay-service': invalidHeader },
    }), /ELF|architecture/);
  }
  assert.throws(() => validatePackage({ ...data, elfHeaders: {} }), /Missing extracted ELF header/);
});
test('oversized executable or package listings are rejected before streaming extraction', () => {
  const data = fixture();
  assert.throws(() => validatePackage({
    ...data,
    contents: data.contents.replace(/0\/0 \d+ /, '0/0 268435457 '),
  }), /bounded inspection size/);
  assert.throws(() => validatePackage({ ...data, contents: `${data.contents}\nnot a dpkg-deb listing` }), /Unrecognized Debian payload listing/);
});
test('unsafe paths, unknown architecture and absent runtime dependencies fail', () => {
  assert.throws(() => validatePackage({ ...fixture(), architecture: 'all' }));
  assert.throws(() => validatePackage({ ...fixture(), depends: '' }));
  assert.throws(() => validatePackage({ ...fixture(), contents: fixture().contents.replace('usr/bin', '../bin') }));
});
test('listing forces numeric ownership and executable extraction disables inherited tar options', async t => {
  const originalTarOptions = process.env.TAR_OPTIONS;
  t.after(() => {
    if (originalTarOptions === undefined) delete process.env.TAR_OPTIONS;
    else process.env.TAR_OPTIONS = originalTarOptions;
  });
  process.env.TAR_OPTIONS = '--checkpoint-action=exec=must-not-run';
  const data = fixture();
  const calls = [];
  const runMetadata = (command, args, options) => {
    calls.push({ command, args, env: options.env });
    return { status: 0, stdout: metadataOutput(args.at(-1), data) };
  };
  const extractionCalls = [];
  const result = await inspectPackage(
    '/fixture/package.deb',
    runMetadata,
    fakeExtractor(payloads(data), extractionCalls, data.contents),
  );
  assert.equal(result.scope, 'metadata-payload-elf');
  assert.deepEqual(result.elfArchitectures, Object.fromEntries(binaries.map(name => [`usr/bin/${name}`, 'AArch64'])));
  assert.ok(calls.every(({ command, args }) => command === 'dpkg-deb'
    && ((args[0] === '--field' && ['Package', 'Version', 'Homepage', 'Maintainer', 'Architecture', 'Depends'].includes(args.at(-1)))
      || (args[0] === '--info' && Object.hasOwn(lifecycleHooks, args.at(-1))))));
  assert.ok(calls.every(({ env }) => !Object.hasOwn(env, 'TAR_OPTIONS')));
  const members = [...binaries.map(name => `./usr/bin/${name}`), `./${receiptPath}`];
  assert.deepEqual(extractionCalls.map(([, args]) => args), [
    ['--fsys-tarfile', '/fixture/package.deb'], ['--numeric-owner', '-tvf', '-'],
    ...members.flatMap(member => [['--fsys-tarfile', '/fixture/package.deb'], ['-xO', '-f', '-', '--', member]]),
  ]);
  assert.deepEqual(extractionCalls.map(([command]) => command),
    Array(members.length + 1).fill(['dpkg-deb', 'tar']).flat());
  assert.ok(extractionCalls.every(([, , options]) => !Object.hasOwn(options.env, 'TAR_OPTIONS')));
});
test('oversized streamed listing kills both owned processes and rejects the package', async () => {
  const data = fixture();
  const kills = [];
  await assert.rejects(inspectPackage(
    '/fixture/package.deb',
    (_command, args) => ({ status: 0, stdout: metadataOutput(args.at(-1), data) }),
    fakeExtractor(new Map(), [], Buffer.alloc(4 * 1024 * 1024 + 1, 0x78), kills),
  ), /payload listing exceeds the bounded inspection size/);
  assert.deepEqual(kills, [['dpkg-deb', 'SIGKILL'], ['tar', 'SIGKILL']]);
});
test('oversized decompressed archive kills both children with bounded fixture memory', async () => {
  const data = fixture();
  const kills = [];
  // Reuse one 16 MiB buffer; the streamed byte count exceeds the 1 GiB-plus-headers cap.
  const chunks = Array(68).fill(Buffer.alloc(16 * 1024 * 1024));
  await assert.rejects(inspectPackage(
    '/fixture/package.deb',
    (_command, args) => ({ status: 0, stdout: metadataOutput(args.at(-1), data) }),
    fakeExtractor(new Map(), [], data.contents, kills, chunks),
  ), /uncompressed payload exceeds the bounded inspection size/);
  assert.deepEqual(kills, [['dpkg-deb', 'SIGKILL'], ['tar', 'SIGKILL']]);
});
test('a stalled package pipeline times out and kills both owned children', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const data = fixture();
  const kills = [];
  const spawnStalled = command => {
    const child = new EventEmitter();
    child.stdin = new PassThrough();
    child.stdout = new PassThrough();
    child.stderr = new PassThrough();
    child.kill = signal => { kills.push([command, signal]); return true; };
    return child;
  };
  const pending = inspectPackage('/fixture/package.deb',
    (_command, args) => ({ status: 0, stdout: metadataOutput(args.at(-1), data) }),
    spawnStalled);
  const rejected = assert.rejects(pending, /inspection timed out/);
  t.mock.timers.tick(120_000);
  await rejected;
  assert.deepEqual(kills, [['dpkg-deb', 'SIGKILL'], ['tar', 'SIGKILL']]);
});
test('inspection preserves exact Debian member spelling with and without a leading ./', async () => {
  for (const prefix of ['./', '']) {
    const data = fixture();
    data.contents = data.contents.replaceAll(' ./usr/', ` ${prefix}usr/`);
    const extractionCalls = [];
    await inspectPackage(
      '/fixture/package.deb',
      (_command, args) => ({ status: 0, stdout: metadataOutput(args.at(-1), data) }),
      fakeExtractor(payloads(data), extractionCalls, data.contents),
    );
    assert.deepEqual(
      extractionCalls.filter(([, args]) => args[0] === '-xO').map(([, args]) => args.at(-1)),
      [...binaries.map(name => `${prefix}usr/bin/${name}`), `${prefix}${receiptPath}`],
    );
  }
});
test('inspection metadata command failures and bounded extraction failures are not passes', async () => {
  await assert.rejects(inspectPackage('/fixture/package.deb', () => ({ status: 1 })), /Cannot read/);
  const data = fixture();
  const truncatedFirstExecutable = new Map([
    ['usr/bin/mixrelay-desktop', elfHeader(183).subarray(0, 32)],
  ]);
  await assert.rejects(inspectPackage('/fixture/package.deb', (command, args) => ({
    status: 0,
    stdout: metadataOutput(args.at(-1), data),
  }), fakeExtractor(truncatedFirstExecutable, [], data.contents)), /byte count does not match/);
});
test('Debian artifact inspection rejects missing or modified lifecycle hooks', async () => {
  const data = fixture();
  for (const script of Object.keys(lifecycleHooks)) {
    await assert.rejects(inspectPackage('/fixture/package.deb', (_command, args) => ({
      status: 0,
      stdout: args.at(-1) === script ? '#!/bin/sh\nexit 0\n' : metadataOutput(args.at(-1), data),
    }), fakeExtractor(payloads(data), [], data.contents)),
    new RegExp(`Missing or mismatched package lifecycle hook: ${script}`));
  }
});

test('the product contains editor, both services, one remover and the same version receipt', () => {
  const config = linuxConfig();
  const product = { ...fixture('amd64'),
    contents: [...binaries.map(name => `-rwxr-xr-x 0/0 64 2026-10-12 00:00 ./usr/bin/${name}`),
      ...Object.keys(config.bundle.linux.deb.files).map(path => `-rw-r--r-- 0/0 64 2026-10-12 00:00 .${path}`)].join('\n'),
  };
  assert.deepEqual(validatePackage(product).executables, binaries.map(name => `usr/bin/${name}`));
  for (const missing of binaries) {
    assert.throws(() => validatePackage({ ...product, contents: product.contents.split('\n').filter(line => !line.includes(`/usr/bin/${missing}`)).join('\n') }), /Missing non-empty regular executable/);
  }
  for (const versionReceipt of [undefined, '{"version":"0.1.1"}', '{"version":"0.1"}']) {
    assert.throws(() => validatePackage({ ...product, versionReceipt }), /receipt/);
  }
  assert.throws(() => validatePackage({ ...product, packageName: 'mix-relay-extras' }), /Unsupported package/);
  assert.equal(config.bundle.linux.deb.recommends, undefined);
});

test('Debian audio dependencies reject missing, weaker and optional version fences', () => {
  for (const [name, minimum] of [['libpipewire-0.3-0t64', '1.2.0'], ['pipewire', '1.2.0'], ['wireplumber', '0.5.0']]) {
    for (const replacement of [name, `${name} (>= 0.3.0)`, `${name} (>= ${minimum}) | unrelated`]) {
      const data = fixture();
      data.depends = data.depends.replace(`${name} (>= ${minimum})`, replacement);
      assert.throws(() => validatePackage(data), /Missing required dependency/);
    }
  }
});

test('stronger audio release floors and equally constrained alternatives remain valid', () => {
  const data = fixture();
  data.depends = data.depends.replace('pipewire (>= 1.2.0)', 'pipewire (>= 1.4.2) | pipewire (= 1.2.7)');
  data.depends = data.depends.replace('wireplumber (>= 0.5.0)', 'wireplumber (>= 0.5.8)');
  assert.doesNotThrow(() => validatePackage(data));
});

test('DEB rejects an unresolved homepage and a maintainer without a contact address', () => {
  for (const change of [{ homepage: 'https://mixrelay.com' }, { homepage: undefined },
    { maintainer: 'MixRelay' }, { maintainer: 'Unknown Packager' }, { maintainer: undefined }]) {
    assert.throws(() => validatePackage({ ...fixture(), ...change }), /Homepage and Maintainer/);
  }
});
