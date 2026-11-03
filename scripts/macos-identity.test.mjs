// The macOS bundle identities of MixRelay's own processes are written in several places.
// System Audio excludes these identities, so a copy that drifts makes it capture the
// product's own output. The bundle definitions are the source; every copy must match them.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { SELF_BUNDLE_IDS } from './e2e/system-capture.mjs';

const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), 'utf8');
const only = (source, pattern, what) => {
  const matches = [...source.matchAll(pattern)].map((match) => match[1]);
  assert.ok(matches.length > 0, `${what} not found`);
  return matches;
};
const strings = (list) => [...list.matchAll(/'([^']*)'/g)].map((match) => match[1]);

const engine = only(
  read('native/macos/service/Info.plist'),
  /<key>CFBundleIdentifier<\/key>\s*<string>([^<]+)<\/string>/g,
  'engine CFBundleIdentifier',
)[0];
const desktop = JSON.parse(read('apps/desktop/src-tauri/tauri.conf.json')).identifier;

test('the engine bundle identity is reverse-DNS and distinct from the app', () => {
  assert.equal(engine, 'io.mixrelay.service');
  assert.equal(desktop, 'io.mixrelay.desktop');
});

test('the installer requires the engine signature to carry the bundle identity', () => {
  assert.deepEqual(
    only(read('native/macos/installer/src/contract.rs'), /RUNTIME_ENGINE_IDENTIFIER: &str = "([^"]+)"/g, 'installer constant'),
    [engine],
  );
  assert.deepEqual(
    only(read('native/macos/installer/src/release/tests.rs'), /\("([^"]+)", ArtifactRole::Engine\)/g, 'installer test identity'),
    [engine],
  );
  // The release build signs with the identifier it reads from the same Info.plist.
  const build = read('scripts/build-macos-universal-engine.sh');
  assert.match(build, /plutil -extract CFBundleIdentifier raw -o - "\$engine_info_plist"/);
  assert.deepEqual(only(build, /--identifier (\S+)/g, 'engine signing identifier'), ['"$engine_identifier"']);
});

test('System Audio excludes exactly the app and engine identities', () => {
  const ui = only(
    read('apps/control-ui/src/features/routing-workspace/routingNodeMenus.ts'),
    /SYSTEM_AUDIO_EXCLUDED_BUNDLE_IDS: readonly string\[\] = \[([^\]]*)\]/g,
    'UI exclusion list',
  );
  assert.deepEqual(ui.map(strings), [[desktop, engine]]);
  assert.deepEqual(
    only(
      read('apps/control-ui/src/features/routing-workspace/RoutingWorkspace.contract.test.tsx'),
      /bundle_ids: \[([^\]]*)\]/g,
      'UI contract expectation',
    ).map(strings),
    [[desktop, engine]],
  );
  assert.deepEqual([...SELF_BUNDLE_IDS], [desktop, engine]);
});
