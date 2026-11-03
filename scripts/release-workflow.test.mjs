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
