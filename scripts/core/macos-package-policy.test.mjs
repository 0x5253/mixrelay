import assert from 'node:assert/strict';
import test from 'node:test';
import { fixedComponentBundlePolicy, verifyPackageSignature } from './macos-package-policy.mjs';

test('component analysis fixes nested bundle identity without allowing relocation outside staged roots', () => {
  const bundles = [{ RootRelativeBundlePath: 'apps/MixRelay.app', ChildBundles: [
    { RootRelativeBundlePath: 'apps/MixRelay.app/Contents/PlugIns/Owned.appex' },
  ] }];
  const result = fixedComponentBundlePolicy(bundles, '/stage', ['/stage/apps/MixRelay.app']);
  assert.equal(result.componentPlist[0].BundleIsRelocatable, false);
  assert.equal(result.componentPlist[0].ChildBundles[0].BundleHasStrictIdentifier, true);
  assert.throws(() => fixedComponentBundlePolicy([{ RootRelativeBundlePath: '../outside.app' }], '/stage', ['/stage']), /unsafe/);
});

test('installer signature requires the supplied leaf and a trusted timestamp', () => {
  const identity = 'Developer ID Installer: Example (ABCDEFGHIJ)';
  const signature = `Status: signed by a certificate trusted by macOS\nCertificate Chain:\n 1. ${identity}\nSigned with a trusted timestamp on: 2026-11-02\n`;
  verifyPackageSignature(signature, identity, 'installer');
  assert.throws(() => verifyPackageSignature(signature, `${identity} other`, 'installer'), /not trusted/);
  assert.throws(() => verifyPackageSignature(signature.replace('Signed with a trusted timestamp on:', 'Signed on:'), identity, 'installer'), /not trusted/);
});
