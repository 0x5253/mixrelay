import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { chmodSync, mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { compareRpmToDeb, extractedFiles, rpmFiles } from './check-rpm-package.mjs';

const digest = 'a'.repeat(64);
const scripts = { PREIN: 'pre\n', PREUN: 'preun\n', POSTIN: 'post\n', POSTUN: 'postun\n' };
const sources = {
  'driver-before-change.sh': 'pre', 'driver-rpm-pre-remove.sh': 'preun',
  'driver-after-install.sh': 'post', 'driver-after-remove.sh': 'postun',
};
const policy = {
  deb: { version: '0.2.0-1', architecture: 'arm64' }, homepage: 'https://rawsector.com/products/mixrelay',
  dependencies: ['pipewire', 'alsa-lib'], source: file => sources[file],
};
const payload = () => new Map([
  ['usr/bin/mixrelay-desktop', { mode: '-rwxr-xr-x', digest }],
  ['usr/share/doc/mix-relay/LICENSE.txt', { mode: '-rw-r--r--', digest: 'b'.repeat(64) }],
]);
const rpm = (overrides = {}) => ({
  metadata: { name: 'mix-relay', version: '0.2.0', release: '1', arch: 'aarch64', url: policy.homepage },
  requires: ['pipewire', 'alsa-lib', 'rpmlib(CompressedFileNames) <= 3.0.4-1'],
  scripts, rpm: payload(), deb: payload(), ...overrides,
});

test('an RPM carrying the checked DEB payload, metadata and scriptlets passes', () => {
  assert.deepEqual(compareRpmToDeb(rpm(), policy),
    { name: 'mix-relay', version: '0.2.0', release: '1', arch: 'aarch64', url: policy.homepage, files: 2 });
});

test('metadata, requirements, scriptlets and every payload difference fail', () => {
  for (const [change, message] of [
    [{ metadata: { ...rpm().metadata, arch: 'x86_64' } }, /RPM arch is x86_64, expected aarch64/],
    [{ metadata: { ...rpm().metadata, version: '0.1.0' } }, /RPM version/],
    [{ metadata: { ...rpm().metadata, url: 'https://example.org' } }, /RPM url/],
    [{ requires: ['pipewire'] }, /Missing RPM requirement: alsa-lib/],
    [{ scripts: { ...scripts, PREUN: 'other' } }, /mismatched RPM scriptlet: PREUN/],
    [{ scripts: { ...scripts, POSTUN: undefined } }, /mismatched RPM scriptlet: POSTUN/],
  ]) assert.throws(() => compareRpmToDeb(rpm(change), policy), message);
  const missing = payload();
  missing.delete('usr/bin/mixrelay-desktop');
  assert.throws(() => compareRpmToDeb(rpm({ rpm: missing }), policy), /RPM payload lacks usr\/bin\/mixrelay-desktop/);
  assert.throws(() => compareRpmToDeb(rpm({ deb: missing }), policy), /DEB payload lacks usr\/bin\/mixrelay-desktop/);
  for (const entry of [{ mode: '-rw-r--r--', digest }, { mode: '-rwxr-xr-x', digest: 'c'.repeat(64) }]) {
    const changed = payload().set('usr/bin/mixrelay-desktop', entry);
    assert.throws(() => compareRpmToDeb(rpm({ rpm: changed }), policy), /RPM payload differs from the DEB: usr\/bin\/mixrelay-desktop/);
  }
});

test('RPM header listings keep files and links, skip directories and require root ownership', () => {
  const files = rpmFiles([
    `drwxr-xr-x\troot:root\t\t/usr/bin`,
    `-rwxr-xr-x\troot:root\t${digest}\t/usr/bin/mixrelay-desktop`,
    `lrwxrwxrwx\troot:root\t\t/usr/bin/mixrelay`,
    '',
  ].join('\n'));
  assert.deepEqual([...files], [
    ['usr/bin/mixrelay-desktop', { mode: '-rwxr-xr-x', digest }],
    ['usr/bin/mixrelay', { mode: 'lrwxrwxrwx', digest: '' }],
  ]);
  assert.throws(() => rpmFiles(`-rw-r--r--\tuser:user\t${digest}\t/etc/x`), /not owned by root: \/etc\/x/);
  assert.throws(() => rpmFiles(`-rw-r--r--\troot:root\t${digest}\tetc/x`), /Invalid RPM file entry/);
  assert.throws(() => rpmFiles(`-rw-r--r--\troot:root\t${digest}\t/etc/x\n-rw-r--r--\troot:root\t${digest}\t/etc/x`),
    /Invalid RPM file entry/);
});

test('an extracted DEB yields file modes and SHA-256 digests in RPM form', t => {
  const directory = mkdtempSync(join(tmpdir(), 'mixrelay-rpm-test-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  mkdirSync(join(directory, 'usr/bin'), { recursive: true });
  writeFileSync(join(directory, 'usr/bin/tool'), 'tool');
  chmodSync(join(directory, 'usr/bin/tool'), 0o755);
  writeFileSync(join(directory, 'usr/bin/data'), '');
  chmodSync(join(directory, 'usr/bin/data'), 0o644);
  symlinkSync('tool', join(directory, 'usr/bin/link'));
  const files = extractedFiles(directory);
  assert.deepEqual(files.get('usr/bin/tool'),
    { mode: '-rwxr-xr-x', digest: createHash('sha256').update('tool').digest('hex') });
  assert.equal(files.get('usr/bin/data').mode, '-rw-r--r--');
  assert.equal(files.get('usr/bin/data').digest, 'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855');
  assert.equal(files.get('usr/bin/link').mode[0], 'l');
  assert.equal(files.get('usr/bin/link').digest, '');
  assert.equal(files.has('usr/bin'), false);
});
