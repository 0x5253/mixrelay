import assert from 'node:assert/strict';
import test from 'node:test';
import { packageMetadata } from './linux-package-policy.mjs';

const data = {
  architecture: 'arm64', packageName: 'mix-relay', homepage: 'https://example.test/product', maintainer: 'Test <test@example.test>',
  depends: 'libpipewire-0.3-0t64 (>= 1.2.0), liblilv-0-0, libgcc-s1, libc6, libwebkit2gtk-4.1-0, libgtk-3-0, librsvg2-common, pipewire (>= 1.2.0), wireplumber (>= 0.5.0), libayatana-appindicator3-1, zenity, pkexec, polkit-1-auth-agent, systemd, util-linux',
  contents: '',
};
const policy = {
  metadata: { homepage: data.homepage, contact: data.maintainer },
  minimums: { linux: { pipewire: '1.2.0', wireplumber: '0.5.0' } },
  releaseDocuments: [],
};

test('package dependency floors come from supplied support facts before payload inspection', () => {
  assert.throws(() => packageMetadata(data, policy), /Missing non-empty regular executable payload/);
  const raised = { ...policy, minimums: { linux: { ...policy.minimums.linux, pipewire: '1.3.0' } } };
  assert.throws(() => packageMetadata(data, raised), /Missing required dependency: libpipewire-0.3-0t64 \(>= 1.3.0\)/);
  assert.throws(() => packageMetadata({ ...data, homepage: 'https://other.test' }, policy), /Homepage and Maintainer/);
});
