import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { compareRpmToDeb, debModes, normalizedContent, payloadDigests, rpmModes } from './check-rpm-package.mjs';

const scripts = { PREIN: 'pre\n', PREUN: 'preun\n', POSTIN: 'post\n', POSTUN: 'postun\n' };
const sources = {
  'driver-before-change.sh': 'pre', 'driver-rpm-pre-remove.sh': 'preun',
  'driver-after-install.sh': 'post', 'driver-after-remove.sh': 'postun',
};
const policy = {
  version: '0.2.0', architecture: 'arm64', homepage: 'https://rawsector.com/products/mixrelay',
  dependencies: ['pipewire', 'alsa-lib'], source: file => sources[file],
};
const desktop = type => Buffer.from(`ELF…__TAURI_BUNDLE_TYPE_VAR_${type}…code`);
const modes = () => new Map([['usr/bin/mixrelay-desktop', '-rwxr-xr-x'], ['usr/share/doc/mix-relay/LICENSE.txt', '-rw-r--r--']]);
const digests = type => new Map([
  ['usr/bin/mixrelay-desktop', normalizedContent(desktop(type), type)],
  ['usr/share/doc/mix-relay/LICENSE.txt', normalizedContent(Buffer.from('licence'), type)],
]);
const rpm = (overrides = {}) => ({
  metadata: { name: 'mix-relay', version: '0.2.0', release: '1', arch: 'aarch64', url: policy.homepage },
  requires: ['pipewire', 'alsa-lib', 'rpmlib(CompressedFileNames) <= 3.0.4-1'],
  scripts,
  modes: { rpm: modes(), deb: modes() },
  digests: { rpm: digests('RPM'), deb: digests('DEB') },
  ...overrides,
});

test('an RPM of the checked DEB payload, differing only in its package-type marker, passes', () => {
  assert.deepEqual(compareRpmToDeb(rpm(), policy),
    { name: 'mix-relay', version: '0.2.0', release: '1', arch: 'aarch64', url: policy.homepage, files: 2 });
});

test('metadata, requirements and scriptlets must match the product', () => {
  for (const [change, message] of [
    [{ metadata: { ...rpm().metadata, arch: 'x86_64' } }, /RPM arch is x86_64, expected aarch64/],
    [{ metadata: { ...rpm().metadata, version: '0.1.0' } }, /RPM version/],
    [{ metadata: { ...rpm().metadata, url: 'https://example.org' } }, /RPM url/],
    [{ requires: ['pipewire'] }, /Missing RPM requirement: alsa-lib/],
    [{ scripts: { ...scripts, PREUN: 'other' } }, /mismatched RPM scriptlet: PREUN/],
    [{ scripts: { ...scripts, POSTUN: undefined } }, /mismatched RPM scriptlet: POSTUN/],
  ]) assert.throws(() => compareRpmToDeb(rpm(change), policy), message);
});

test('any payload difference beyond the package-type marker fails', () => {
  const missing = modes();
  missing.delete('usr/share/doc/mix-relay/LICENSE.txt');
  assert.throws(() => compareRpmToDeb(rpm({ modes: { rpm: missing, deb: modes() } }), policy),
    /RPM payload lacks usr\/share\/doc\/mix-relay\/LICENSE.txt/);
  assert.throws(() => compareRpmToDeb(rpm({ modes: { rpm: modes(), deb: missing } }), policy),
    /DEB payload lacks usr\/share\/doc\/mix-relay\/LICENSE.txt/);
  const writable = modes().set('usr/bin/mixrelay-desktop', '-rwxrwxrwx');
  assert.throws(() => compareRpmToDeb(rpm({ modes: { rpm: writable, deb: modes() } }), policy), /differs from the DEB: usr\/bin/);
  const changed = digests('RPM').set('usr/bin/mixrelay-desktop', normalizedContent(Buffer.from('ELF…other code'), 'RPM'));
  assert.throws(() => compareRpmToDeb(rpm({ digests: { rpm: changed, deb: digests('DEB') } }), policy), /differs from the DEB: usr\/bin/);
  // A DEB-marked binary in the RPM would misreport its package type at runtime.
  const unmarked = digests('RPM').set('usr/bin/mixrelay-desktop', normalizedContent(desktop('DEB'), 'RPM'));
  assert.throws(() => compareRpmToDeb(rpm({ digests: { rpm: unmarked, deb: digests('DEB') } }), policy), /differs from the DEB/);
});

test('the RPM desktop binary must record its package type exactly once', () => {
  const twice = Buffer.concat([desktop('RPM'), desktop('RPM')]);
  const both = {
    rpm: digests('RPM').set('usr/bin/mixrelay-desktop', normalizedContent(twice, 'RPM')),
    deb: digests('DEB').set('usr/bin/mixrelay-desktop', normalizedContent(Buffer.concat([desktop('DEB'), desktop('DEB')]), 'DEB')),
  };
  assert.throws(() => compareRpmToDeb(rpm({ digests: both }), policy), /record its package type once/);
});

test('marker normalisation leaves other bytes intact and counts markers', () => {
  const normalized = normalizedContent(desktop('RPM'), 'RPM');
  assert.deepEqual(normalized, normalizedContent(desktop('DEB'), 'DEB'));
  assert.equal(normalized.markers, 1);
  assert.equal(normalizedContent(Buffer.from('plain'), 'RPM').markers, 0);
  assert.notEqual(normalizedContent(desktop('RPM'), 'DEB').digest, normalized.digest);
});

test('RPM header and DEB listings yield root-owned file modes and skip directories', () => {
  assert.deepEqual([...rpmModes([
    'drwxr-xr-x\troot:root\t/usr/bin',
    '-rwxr-xr-x\troot:root\t/usr/bin/mixrelay-desktop',
    'lrwxrwxrwx\troot:root\t/usr/bin/mixrelay',
    '',
  ].join('\n'))], [['usr/bin/mixrelay-desktop', '-rwxr-xr-x'], ['usr/bin/mixrelay', 'lrwxrwxrwx']]);
  assert.throws(() => rpmModes('-rw-r--r--\tuser:user\t/etc/x'), /not owned by root: etc\/x/);
  assert.throws(() => rpmModes('-rw-r--r--\troot:root\tetc/x'), /Invalid package file entry/);
  assert.throws(() => rpmModes('-rw-r--r--\troot:root\t/etc/x\n-rw-r--r--\troot:root\t/etc/x'), /Invalid package file entry/);
  assert.deepEqual([...debModes([
    'drwxr-xr-x root/root         0 2026-11-04 01:00 ./',
    'drwxr-xr-x root/root         0 2026-11-04 01:00 ./usr/bin/',
    '-rwxr-xr-x root/root  28000000 2026-11-04 01:00 ./usr/bin/mixrelay-desktop',
    'lrwxrwxrwx root/root         0 2026-11-04 01:00 ./usr/bin/mixrelay -> mixrelay-desktop',
  ].join('\n'))], [['usr/bin/mixrelay-desktop', '-rwxr-xr-x'], ['usr/bin/mixrelay', 'lrwxrwxrwx']]);
  // Tauri's archives use numeric owners and no "./" prefix.
  assert.deepEqual([...debModes([
    'drwxr-xr-x 0/0               0 2026-11-04 03:29 etc',
    '-rw-r--r-- 0/0              10 2026-11-04 03:29 etc/xdg/autostart/mixrelay-driver.desktop',
  ].join('\n'))], [['etc/xdg/autostart/mixrelay-driver.desktop', '-rw-r--r--']]);
  assert.throws(() => debModes('-rw-r--r-- 1000/1000 10 2026-11-04 03:29 etc/x'), /not owned by root/);
  assert.throws(() => debModes('garbage line'), /Unrecognized DEB listing entry/);
});

test('extracted payload digests cover regular files only', t => {
  const directory = mkdtempSync(join(tmpdir(), 'mixrelay-rpm-test-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  mkdirSync(join(directory, 'usr/bin'), { recursive: true });
  writeFileSync(join(directory, 'usr/bin/mixrelay-desktop'), desktop('RPM'));
  const files = payloadDigests(directory, 'RPM');
  assert.deepEqual([...files.keys()], ['usr/bin/mixrelay-desktop']);
  assert.deepEqual(files.get('usr/bin/mixrelay-desktop'), normalizedContent(desktop('DEB'), 'DEB'));
});
