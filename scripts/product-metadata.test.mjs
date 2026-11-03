import assert from 'node:assert/strict';
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { tmpdir } from 'node:os';
import test from 'node:test';
import { loadContract } from './support-contract.mjs';
import { syncProductMetadata } from './product-metadata.mjs';
import { validateProductMetadata } from './core/product-metadata-policy.mjs';

test('support metadata generates npm, Cargo and Tauri fields and detects drift', t => {
  const directory = mkdtempSync(join(tmpdir(), 'mixrelay-metadata-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  for (const name of ['package.json', 'apps/control-ui/package.json', 'apps/desktop-shell/package.json',
    'apps/streamdeck-plugin/package.json', 'Cargo.toml', 'apps/desktop-shell/src-tauri/tauri.conf.json']) {
    mkdirSync(dirname(join(directory, name)), { recursive: true });
    cpSync(new URL(`../${name}`, import.meta.url), join(directory, name));
  }
  const metadata = { ...loadContract().contract.metadata, publisher: 'Different Publisher', email: 'other@rawsector.com' };
  assert.throws(() => syncProductMetadata(metadata, directory), /drift/);
  syncProductMetadata(metadata, directory, true);
  assert.doesNotThrow(() => syncProductMetadata(metadata, directory));
  const cargo = readFileSync(join(directory, 'Cargo.toml'), 'utf8');
  assert.match(cargo, /authors = \["Different Publisher <other@rawsector.com>"\]/);
  assert.ok(cargo.includes(`homepage = "${metadata.homepage}"`));
  const npm = JSON.parse(readFileSync(join(directory, 'apps/desktop-shell/package.json')));
  assert.equal(npm.author, 'Different Publisher <other@rawsector.com>');
  assert.equal(npm.homepage, metadata.homepage);
  assert.equal(npm.repository, undefined);
  const tauri = JSON.parse(readFileSync(join(directory, 'apps/desktop-shell/src-tauri/tauri.conf.json')));
  assert.equal(tauri.bundle.publisher, metadata.publisher);
  assert.equal(tauri.bundle.homepage, metadata.homepage);
  writeFileSync(join(directory, 'Cargo.toml'), cargo.replace(/^authors = .*$/m, 'authors = ["MixRelay"]'));
  assert.throws(() => syncProductMetadata(metadata, directory), /Cargo.toml/);
});

test('product metadata cannot inject shell, TOML or package fields', () => {
  const metadata = loadContract().contract.metadata;
  for (const change of [{ homepage: "https://rawsector.com/';false" },
    { publisher: 'Publisher\nInjected' }, { email: 'support@rawsector.com>\nInjected' },
    { publisher: undefined }, { email: undefined }, { homepage: undefined }]) {
    assert.throws(() => validateProductMetadata({ ...metadata, ...change }), /Invalid/);
  }
});
