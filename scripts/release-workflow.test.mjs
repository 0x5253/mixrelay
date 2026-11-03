import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import test from 'node:test';

const workflow = name => readFileSync(new URL(`../.github/workflows/${name}.yml`, import.meta.url), 'utf8');

test('workflow references match the free product and the public repository', () => {
  for (const name of readdirSync(new URL('../.github/workflows/', import.meta.url)).filter(name => name.endsWith('.yml'))) {
    const source = readFileSync(new URL(`../.github/workflows/${name}`, import.meta.url), 'utf8');
    assert.doesNotMatch(source, /mixrelay\/mixrelay|MIXRELAY_POLAR|polar-configuration|installer-configuration|linux-dbus-link/);
    assert.doesNotMatch(source, /push:\s*\n\s*branches:\s*\[main\]/);
  }
});

test('official release builds only a verified version tag and publishes all checked artifacts', () => {
  const source = workflow('release');
  assert.match(source, /tags: \['v\*'\]/);
  assert.match(source, /workflow_dispatch:/);
  assert.match(source, /github\.repository == '0x5253\/mixrelay'/);
  assert.match(source, /product-version\.mjs check/);
  assert.match(source, /git merge-base --is-ancestor "\$REVISION" origin\/main/);
  assert.match(source, /source_ref: \$\{\{ needs\.source\.outputs\.revision \}\}/);
  assert.match(source, /needs: \[source, macos, linux\]/);
  for (const file of ['MixRelay-Installer-$RELEASE_VERSION.pkg', 'MixRelay-StreamDeck-$RELEASE_VERSION.streamDeckPlugin',
    'MixRelay_${RELEASE_VERSION}_amd64.deb', 'MixRelay_${RELEASE_VERSION}_arm64.deb', 'mix-relay-${RELEASE_VERSION}-1-x86_64.pkg.tar.zst']) assert.ok(source.includes(file), file);
  assert.match(source, /sha256sum --check SHA256SUMS-/);
  assert.match(source, /sha256sum \* > SHA256SUMS && sha256sum --check SHA256SUMS/);
  assert.match(source, /gh release create .*--verify-tag --draft/);
  assert.match(source, /gh release edit .*--draft=false --prerelease=false --latest/);
  assert.match(source, /\.draft == true and \.prerelease == false/);
  assert.doesNotMatch(source, /--skip-notarization|secrets: inherit|windows/i);
});

test('signing assets are scoped to the macOS build and cleaned before uploading only distributables', () => {
  const source = workflow('release');
  const macos = source.slice(source.indexOf('\n  macos:'), source.indexOf('\n  publish:'));
  assert.match(macos, /prepare-ci-macos-signing\.mjs --output candidate/);
  assert.match(macos, /\n    environment: release\n/);
  assert.equal(source.match(/environment: release/g).length, 1);
  assert.doesNotMatch(source, /pull_request/);
  assert.match(macos, /if: always\(\)\n\s*run: node scripts\/prepare-ci-macos-signing\.mjs --cleanup/);
  assert.ok(macos.indexOf('--cleanup') < macos.indexOf('actions/upload-artifact@'));
  assert.match(macos, /"notarized-artifacts-built"/);
  assert.match(macos, /pkgutil --check-signature/);
  assert.match(macos, /stapler validate/);
  assert.doesNotMatch(macos, /path:.*(?:keychain|p12|p8|config\.json)/);
  assert.doesNotMatch(source.slice(source.indexOf('\n  publish:')), /secrets\.MIXRELAY/);
});

test('Linux release reuses inspected native-CPU packages and the checked Arch payload', () => {
  const source = workflow('build-linux-release');
  assert.match(source, /workflow_call:/);
  assert.match(source, /linuxBuild/);
  assert.match(source, /inputs\.source_ref \|\| github\.sha/);
  assert.match(source, /pnpm desktop:build --bundles deb/);
  assert.match(source, /dpkg-deb --field "\$package" Version/);
  assert.match(source, /prepare-arch-driver-package\.mjs/);
  assert.match(source, /makepkg --nodeps --noconfirm/);
  assert.match(source, /bsdtar -xOf "\$package" \.PKGINFO/);
  assert.match(source, /sha256sum \*\.deb/);
  assert.match(source, /sha256sum \*\.pkg\.tar\.zst/);
});

test('docs, ci and chore pull requests run no build, test or release workflow', async () => {
  const { TYPES, NON_BUILD_TYPES } = await import('./check-conventional-title.mjs');
  // startsWith gates stay exact only while no other title type shares a prefix with a skipped type.
  assert.deepEqual(TYPES.filter(type => NON_BUILD_TYPES.some(skip => type !== skip && type.startsWith(skip))), []);
  const gate = NON_BUILD_TYPES.map(type => `!startsWith(github.event.pull_request.title, '${type}')`).join(' && ');
  for (const name of readdirSync(new URL('../.github/workflows/', import.meta.url)).filter(name => name.endsWith('.yml'))) {
    const source = readFileSync(new URL(`../.github/workflows/${name}`, import.meta.url), 'utf8');
    assert.doesNotMatch(source, /pull_request_target|workflow_run:/, name);
    if (!/^\s+pull_request:/m.test(source) || name === 'pr-title.yml') continue;
    const jobs = source.slice(source.indexOf('\njobs:\n')).split(/\n  (?=[a-z][a-z0-9_-]*:\n)/).slice(1);
    for (const job of jobs.filter(job => !/\n    needs:/.test(job))) {
      assert.ok(job.includes(gate), `${name}: ${job.split(':')[0]} must skip ${NON_BUILD_TYPES.join(', ')} pull requests`);
    }
  }
});

test('every bundled resource is inside the Linux build container context', () => {
  const root = new URL('../', import.meta.url);
  const allowed = readFileSync(new URL('.dockerignore', root), 'utf8').split('\n')
    .filter(line => line.startsWith('!')).map(line => line.slice(1))
    .map(pattern => new RegExp(`^${pattern.replace(/[.+^${}()|[\]\\]/g, '\\$&').replace(/\*\*/g, '.*').replace(/(?<!\.)\*/g, '[^/]*')}$`));
  const fromTauri = (file, base) => {
    const config = JSON.parse(readFileSync(new URL(`apps/desktop/src-tauri/${file}`, root), 'utf8'));
    const paths = [...Object.keys(config.bundle?.resources ?? {})];
    for (const format of Object.values(config.bundle?.linux ?? {})) paths.push(...Object.values(format.files ?? {}));
    return paths.map(path => new URL(path, base).pathname.slice(new URL(root).pathname.length));
  };
  const resources = [...fromTauri('tauri.conf.json', new URL('apps/desktop/src-tauri/', root)),
    ...fromTauri('tauri.linux.conf.json', new URL('apps/desktop/src-tauri/', root))].filter(path => !path.startsWith('target/'));
  assert.ok(resources.includes('third-party/THIRD-PARTY-NOTICES.txt'));
  for (const path of resources) assert.ok(allowed.some(pattern => pattern.test(path)), `${path} is excluded by .dockerignore`);
});

test('pull requests build every product unsigned and never reach signing or notarization', () => {
  const source = workflow('pr-build');
  assert.match(source, /^on:\n  pull_request:\n  workflow_dispatch:\n\n/m);
  assert.match(source, /uses: \.\/\.github\/workflows\/build-linux-release\.yml/);
  assert.match(source, /APPLE_SIGNING_IDENTITY: '-'\n\s+MIXRELAY_CODESIGN_IDENTITY: '-'/);
  assert.match(source, /zsh scripts\/build-macos-universal-app\.sh/);
  assert.match(source, /mixrelay-hal-bundle/);
  const code = source.split('\n').filter(line => !line.trimStart().startsWith('#')).join('\n');
  assert.doesNotMatch(code, /secrets\.|vars\.|environment:|prepare-ci-macos-signing|release-macos|notar|productbuild|contents: write/i);
  // Only release.yml may reach the signing environment.
  for (const name of readdirSync(new URL('../.github/workflows/', import.meta.url)).filter(name => name.endsWith('.yml') && name !== 'release.yml')) {
    assert.doesNotMatch(readFileSync(new URL(`../.github/workflows/${name}`, import.meta.url), 'utf8'), /secrets\.|environment: release/, name);
  }
});

test('macOS builds select Xcode 26 before compiling Icon Composer assets', () => {
  for (const name of ['release', 'pr-build']) {
    const source = workflow(name);
    const select = source.indexOf('ls -d /Applications/Xcode_26*.app');
    assert.ok(select > 0, name);
    assert.ok(source.indexOf('DEVELOPER_DIR=$xcode/Contents/Developer" >> "$GITHUB_ENV"') > select, name);
    assert.ok(select < source.indexOf('Install the universal build toolchain'), name);
  }
});
