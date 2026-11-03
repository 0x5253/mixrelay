import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';
import { checkPublicDestinations } from './check-public-destinations.mjs';
import { assertPublicDestination, assertPublicReferences, extractDestinations, isUserFacingSurface, publicSurfaceText } from './core/public-references.mjs';

test('every customer surface uses explicitly approved public destinations', () => {
  assert.ok(checkPublicDestinations() > 0);
});

test('unapproved repositories, the unresolved domain and anonymous package attribution fail', () => {
  for (const [surface, source] of [
    ['apps/website/index.html', '<a href="https://github.com/other-project/mixrelay/releases">Download</a>'],
    ['packaging/linux/arch/PKGBUILD', "url='https://mixrelay.com'"],
    ['package.PKGINFO', 'packager = Unknown Packager'],
  ]) assert.throws(() => assertPublicReferences(source, surface), /unapproved|anonymous/);
  assert.throws(() => assertPublicReferences('Open MixRelay-StreamDeck-0.1.0.streamDeckPlugin', 'README.md'), /Marketplace/);
});

test('new hosts, deceptive hosts, protocols and unapproved mailboxes fail', () => {
  for (const destination of ['https://new.example.org', 'https://polar.sh/legal/privacy-policy', 'https://buy.polar.sh/polar_cl_fixture', 'https://rawsector.com.attacker.invalid',
    'http://rawsector.com',
    'https://support.apple.com@attacker.invalid', 'https://user@rawsector.com',
    'https://rawsector.com:8443', 'ftp://rawsector.com/file', 'mailto:unknown@rawsector.com',
    'https://downloads.rawsector.com/MixRelay-StreamDeck-0.1.0.streamDeckPlugin']) {
    assert.throws(() => assertPublicDestination(destination), /unapproved/);
  }
});

test('approved product, source, support and legal references pass', () => {
  for (const destination of ['https://rawsector.com/products/mixrelay', 'https://appserver.rawsector.com/products',
    'https://downloads.rawsector.com/mix-relay.pkg', 'https://github.com/0x5253/mixrelay',
    'https://github.com/0x5253/mixrelay/releases',
    'https://support.apple.com/en-gb/guide/mac-help', 'https://www.law.go.kr/LSW/index.html',
    'https://www.easylaw.go.kr/CSP/Main.laf', 'mailto:support@rawsector.com']) {
    assert.doesNotThrow(() => assertPublicDestination(destination));
  }
  for (const retired of ['legal@dsub.io', 'privacy@dsub.io']) {
    assert.throws(() => assertPublicDestination(retired), /unapproved/);
  }
});

test('desktop updates use only the exact public release API and version-matched installer assets', () => {
  const endpoint = 'https://api.github.com/repos/0x5253/mixrelay/releases/latest';
  const source = readFileSync(new URL('../apps/desktop/src-tauri/src/updates.rs', import.meta.url), 'utf8');
  assert.ok(source.includes(endpoint));
  assert.doesNotThrow(() => assertPublicReferences(source, 'apps/desktop/src-tauri/src/updates.rs'));
  for (const file of ['MixRelay-Installer-1.2.3.pkg', 'MixRelay_1.2.3_amd64.deb',
    'MixRelay_1.2.3_arm64.deb', 'mix-relay-1.2.3-1-x86_64.pkg.tar.zst']) {
    const url = `https://github.com/0x5253/mixrelay/releases/download/v1.2.3/${file}`;
    assert.doesNotThrow(() => assertPublicDestination(url));
    assert.throws(() => assertPublicDestination(url.replace('/v1.2.3/', '/v1.2.4/')), /unapproved/);
    assert.throws(() => assertPublicDestination(`${url}?download=1`), /unapproved/);
  }
  for (const url of [endpoint + '?redirect=other', endpoint + '#fragment',
    endpoint.replace('/latest', ''), endpoint.replace('/0x5253/', '/other/'),
    'https://github.com/0x5253/mixrelay/releases/download/v1.2.3/other.pkg',
    'https://github.com/0x5253/mixrelay/releases/download/v1.2.3/MixRelay-Installer-1.2.3.pkg/extra']) {
    assert.throws(() => assertPublicDestination(url), /unapproved/);
  }
});

test('extraction includes plain emails, encoded HTML links and adjacent translated prose', () => {
  assert.deepEqual(extractDestinations('`support@rawsector.com` https://appserver.rawsector.comに接続します。\n'
    + '<a href="https://www.law.go.kr/a?x=1&amp;y=2">law</a> giovanni&#64;fragcolor.com'),
  ['https://appserver.rawsector.com', 'https://www.law.go.kr/a?x=1&y=2', 'support@rawsector.com', 'giovanni@fragcolor.com']);
  assert.deepEqual(extractDestinations('"icons/128x128@2x.png"'), []);
  assert.throws(() => assertPublicReferences('<p>Contact nobody@example.org.</p>', 'manual'), /unapproved/);
});

test('scope includes each shipped boundary and excludes tests, developer docs and evidence', () => {
  for (const path of ['apps/control-ui/src/features/settings/AboutDialog.tsx', 'apps/control-ui/src/core/i18n/locales/en.ts',
    'apps/desktop/src-tauri/src/status_menu/menu.rs', 'native/macos/installer/src/native/dialogs.m',
    'LICENSE.txt', 'third-party/alsa-lib/NOTICE', 'packaging/localization/installer-notices.json',
    'packaging/linux/arch/PKGBUILD.in', 'packaging/linux/mixrelay-uninstall.desktop',
    'apps/streamdeck-plugin/manifest.json', 'apps/streamdeck-plugin/ui/index.html',
    'apps/website/index.html', 'mix-relay.release.json']) assert.ok(isUserFacingSurface(path), path);
  for (const path of ['scripts/check-public-destinations.test.mjs', 'apps/desktop/src-tauri/src/updates_tests.rs',
    'native/macos/installer/src/tests.rs', 'scripts/e2e/qualification.fixture.mjs', 'docs/product.md',
    'docs/validation/evidence/mix-relay.release.json', 'apps/control-ui/src/test-support/integration.ts',
    'apps/control-ui/src/features/settings/AboutDialog.stories.tsx', 'apps/website/assets/fonts/sources.json']) {
    assert.ok(!isUserFacingSurface(path), path);
  }
});

test('schema identifiers and inline tests are excluded while production strings remain checked', () => {
  const text = publicSurfaceText('let url = "https://rawsector.com/products/mixrelay";\n'
    + '#[cfg(test)]\nmod tests {\n    let fixture = "https://example.invalid";\n}\n'
    + 'let support = "mailto:nobody@example.org";', 'native/dialog.rs');
  assert.ok(!text.includes('example.invalid'));
  assert.throws(() => assertPublicReferences(text, 'native/dialog.rs'), /nobody/);
  assert.doesNotThrow(() => assertPublicReferences(publicSurfaceText(
    '<!DOCTYPE plist PUBLIC "plist" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">'
    + '<svg xmlns="http://www.w3.org/2000/svg"/>', 'Info.plist'), 'Info.plist'));
});
