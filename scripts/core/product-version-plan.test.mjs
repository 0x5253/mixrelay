import assert from 'node:assert/strict';
import test from 'node:test';
import { PRODUCT_JSON_MANIFESTS, HAL_MANIFEST, planVersion, checkVersionSnapshot } from './product-version-plan.mjs';

function fixture() {
  return new Map([
    ['Cargo.toml', '[workspace]\nmembers = ["crates/a"]\n[workspace.package]\nversion = "1.0.0"\n'],
    ['crates/a/Cargo.toml', '[package]\nname = "a"\nversion.workspace = true\n'],
    ['Cargo.lock', '[[package]]\nname = "a"\nversion = "1.0.0"\n\n[[package]]\nname = "dependency"\nversion = "1.0.0"\nsource = "registry+https://example.test"\n'],
    [HAL_MANIFEST, '<key>CFBundleShortVersionString</key><string>1.0.0</string><key>CFBundleVersion</key><string>1.0.0</string>'],
    ...PRODUCT_JSON_MANIFESTS.map((name) => [name, '{ "version" : "1.0.0", "description": "fixture" }']),
  ]);
}

test('version plan preserves unusual JSON bytes on no-op and validates all updates before publication', () => {
  const sources = fixture();
  assert.equal(planVersion(sources, '1.0.0').size, 0);
  const edits = planVersion(sources, '1.1.0');
  assert.equal(checkVersionSnapshot(new Map([...sources, ...edits])), '1.1.0');
  assert.equal(sources.get('package.json'), '{ "version" : "1.0.0", "description": "fixture" }');
  assert.match(edits.get('Cargo.lock'), /name = "dependency"\nversion = "1.0.0"/);
  const invalid = new Map(sources);
  invalid.set(HAL_MANIFEST, '<plist/>');
  assert.throws(() => planVersion(invalid, '1.1.0'), /missing CFBundle/);
  assert.equal(sources.get('Cargo.toml'), fixture().get('Cargo.toml'));
});
