import assert from 'node:assert/strict';
import test from 'node:test';
import { CRATES, LAYERS, engineAdapter, packageFeatureViolations, violation, violations } from './check-crate-layers.mjs';

const metadata = (edges, names = Object.keys(LAYERS)) => ({
  packages: names.map((name) => ({
    name,
    dependencies: edges
      .filter(([from]) => from === name)
      .map(([, to, kind, features]) => ({ name: to, kind: kind ?? null, features: features ?? [] })),
  })),
});

test('dependencies the table lists are allowed', () => {
  for (const [from, to] of [
    ['mixrelay-audio-graph', 'mixrelay-audio-io'],
    ['mixrelay-render', 'mixrelay-audio-graph'],
    ['mixrelay-control-protocol', 'mixrelay-audio-graph'],
    ['mixrelay-control-server', 'mixrelay-control-protocol'],
    ['mixrelay-processor-host', 'mixrelay-control-protocol'],
    ['mixrelay-processor-host', 'truce-rack-vst3'],
    ['mixrelay-macos-hal', 'mixrelay-macos-transport'],
    ['mixrelay-macos-hal', 'mixrelay-pcm-ring'],
    ['mixrelay-macos-driver', 'mixrelay-driver-core'],
    ['mixrelay-linux-driver', 'mixrelay-driver-core'],
    ['mixrelay-linux-driver', 'mixrelay-linux-pipewire'],
    ['mixrelay-engine-service', 'mixrelay-linux-pipewire'],
    ['mixrelay-engine-service', 'truce-rack-core'],
    ['mixrelay-engine-service', 'mixrelay-engine-core'],
    ['mixrelay-render', 'mixrelay-time'],
    ['mixrelay-engine-core', 'mixrelay-time'],
    ['mixrelay-engine-core', 'mixrelay-audio-graph'],
    ['mixrelay-engine-core', 'mixrelay-control-protocol'],
    ['mixrelay-engine-core', 'mixrelay-driver-core'],
    ['mixrelay-driver-adapters', 'mixrelay-driver-core'],
    ['mixrelay-macos-installer', 'mixrelay-driver-adapters'],
  ]) {
    assert.equal(violation(from, to, 'normal'), undefined, `${from} -> ${to}`);
  }
});

test('each crate gets exactly its row: no blanket layer allowances', () => {
  // Adapters with no contract in their row may not use one, even though other adapters do.
  for (const from of ['mixrelay-macos-transport', 'mixrelay-macos-hal', 'mixrelay-macos-process-tap', 'mixrelay-linux-pipewire']) {
    assert.match(violation(from, 'mixrelay-driver-core', 'normal'), /not in .* allowed/, from);
  }
  assert.match(violation('mixrelay-macos-installer', 'mixrelay-macos-transport', 'normal'), /not in .* allowed/);
  assert.match(violation('mixrelay-macos-hal-bundle', 'mixrelay-audio-io', 'normal'), /not in .* allowed/);
  assert.match(violation('mixrelay-driver-core', 'mixrelay-driver-adapters', 'normal'), /not in .* allowed/);
  assert.match(violation('mixrelay-engine-core', 'mixrelay-driver-adapters', 'normal'), /not in .* allowed/);
  assert.match(violation('mixrelay-driver-adapters', 'mixrelay-engine-core', 'normal'), /not in .* allowed/);
  assert.match(violation('mixrelay-driver-core', 'mixrelay-audio-graph', 'normal'), /not in .* allowed/);
  assert.match(violation('mixrelay-audio-graph', 'mixrelay-audio-recorder', 'normal'), /not in .* allowed/);
  // The engine reaches recording only through its own sink port.
  assert.match(violation('mixrelay-render', 'mixrelay-audio-recorder', 'normal'), /not in .* allowed/);
  assert.match(violation('mixrelay-control-protocol', 'mixrelay-driver-core', 'normal'), /not in .* allowed/);
  // Build dependencies follow the same row.
  assert.match(violation('mixrelay-audio-io', 'mixrelay-realtime', 'build'), /not in .* allowed/);
});

test('upward, cross-platform, app and test-support dependencies are rejected', () => {
  assert.match(violation('mixrelay-audio-io', 'mixrelay-audio-graph', 'normal'), /not in .* allowed/);
  assert.match(violation('mixrelay-render', 'mixrelay-macos-transport', 'normal'), /not in .* allowed/);
  assert.match(violation('mixrelay-linux-pipewire', 'mixrelay-macos-transport', 'normal'), /not in .* allowed/);
  assert.match(violation('mixrelay-control-server', 'mixrelay-engine-service', 'normal'), /app/);
  assert.match(violation('mixrelay-desktop-shell', 'mixrelay-engine-service', 'normal'), /app/);
  assert.match(violation('mixrelay-audio-io', 'mixrelay-test-support', 'normal'), /dev-dependency/);
  assert.match(violation('mixrelay-engine-service', 'mixrelay-macos-test-support', 'normal'), /dev-dependency/);
  assert.equal(violation('mixrelay-audio-io', 'mixrelay-test-support', 'dev'), undefined);
  assert.match(violation('mixrelay-render', 'truce-rack-core', 'normal'), /not in .* allowed/);
  assert.match(violation('mixrelay-control-server', 'truce-rack-core', 'normal'), /not in .* allowed/);
  assert.match(violation('mixrelay-new', 'mixrelay-audio-io', 'normal'), /no row/);
  assert.match(violation('mixrelay-audio-io', 'mixrelay-new', 'normal'), /no row/);
  // Dev dependencies follow the same table; only test support is added.
  assert.match(violation('mixrelay-audio-io', 'mixrelay-desktop-shell', 'dev'), /app/);
  assert.match(violation('mixrelay-audio-graph', 'mixrelay-linux-pipewire', 'dev'), /not in .* allowed/);
  assert.match(violation('mixrelay-test-support', 'mixrelay-audio-io', 'normal'), /not in .* allowed/);
});

test('every workspace crate needs a row, even one with no dependencies, and rows must be members', () => {
  const names = [...Object.keys(LAYERS), 'mixrelay-unregistered'];
  assert.deepEqual(violations(metadata([], names)), [
    'mixrelay-unregistered: workspace crate has no row in the crate dependency table and scripts/check-crate-layers.mjs',
  ]);
  const missing = Object.keys(LAYERS).filter((name) => name !== 'mixrelay-time');
  assert.deepEqual(violations(metadata([], missing)), [
    'mixrelay-time: listed in scripts/check-crate-layers.mjs but not a workspace crate',
  ]);
});

test('the report lists each forbidden edge once and names its dependency kind', () => {
  const found = violations(metadata([
    ['mixrelay-audio-io', 'mixrelay-render'],
    ['mixrelay-audio-io', 'mixrelay-render', 'build'],
    ['mixrelay-audio-graph', 'mixrelay-audio-io'],
    ['mixrelay-macos-hal', 'mixrelay-driver-core', 'dev'],
  ]));
  assert.deepEqual(found, [
    "mixrelay-audio-io -> mixrelay-render (build): not in mixrelay-audio-io's allowed dependencies in the crate dependency table",
    "mixrelay-audio-io -> mixrelay-render (normal): not in mixrelay-audio-io's allowed dependencies in the crate dependency table",
    "mixrelay-macos-hal -> mixrelay-driver-core (dev): not in mixrelay-macos-hal's allowed dependencies in the crate dependency table",
  ]);
});

test('allow-lists only name registered crates', () => {
  for (const [name, row] of Object.entries(CRATES)) {
    if (!Array.isArray(row.allows)) continue;
    for (const to of row.allows) assert.ok(CRATES[to], `${name} allows unregistered ${to}`);
  }
});

test('the engine domain sits above model and contract crates, beside service parts and below adapters and apps', () => {
  for (const to of ['mixrelay-macos-transport', 'mixrelay-linux-pipewire', 'truce-rack-core']) {
    assert.match(violation('mixrelay-engine-core', to, 'normal'), /not in .* allowed/, to);
  }
  assert.match(violation('mixrelay-engine-core', 'mixrelay-engine-service', 'normal'), /app/);
  // Inner crates define the ports they use instead of reaching up to the domain crate.
  for (const from of ['mixrelay-time', 'mixrelay-render', 'mixrelay-control-protocol']) {
    assert.match(violation(from, 'mixrelay-engine-core', 'normal'), /not in .* allowed/, from);
  }
  // The domain crate and the service parts are peers: neither depends on the other.
  for (const part of ['mixrelay-control-server', 'mixrelay-processor-host']) {
    assert.match(violation('mixrelay-engine-core', part, 'normal'), /not in .* allowed/, part);
    assert.match(violation(part, 'mixrelay-engine-core', 'normal'), /not in .* allowed/, part);
  }
  assert.match(violation('mixrelay-time', 'mixrelay-audio-io', 'normal'), /not in .* allowed/);
});

test('an adapter row names only adapters of its own OS', () => {
  const os = { macos: 'linux', linux: 'macos' };
  for (const [name, row] of Object.entries(CRATES)) {
    if (!(row.layer in os) || !Array.isArray(row.allows)) continue;
    for (const to of row.allows) assert.notEqual(CRATES[to].layer, os[row.layer], `${name} allows ${to}`);
  }
  // Engine adapter rows built with the helper follow the same rule.
  const linuxEngine = { layer: 'linux', allows: engineAdapter('mixrelay-linux-pipewire') };
  assert.ok(linuxEngine.allows.every((to) => CRATES[to].layer !== 'macos'));
});

test('engine adapters may use mixrelay-engine; other adapters may not', () => {
  // No engine adapter exists yet, so check rows shaped like the ones that will be added.
  const crates = {
    ...CRATES,
    'mixrelay-macos-engine': { layer: 'macos', allows: engineAdapter('mixrelay-macos-transport', 'mixrelay-macos-process-tap') },
    'mixrelay-linux-engine': { layer: 'linux', allows: engineAdapter('mixrelay-linux-pipewire') },
  };
  for (const [from, to] of [
    ['mixrelay-macos-engine', 'mixrelay-engine-core'],
    ['mixrelay-macos-engine', 'mixrelay-time'],
    ['mixrelay-macos-engine', 'mixrelay-audio-graph'],
    ['mixrelay-macos-engine', 'mixrelay-driver-core'],
    ['mixrelay-macos-engine', 'mixrelay-macos-process-tap'],
    ['mixrelay-linux-engine', 'mixrelay-engine-core'],
    ['mixrelay-linux-engine', 'mixrelay-linux-pipewire'],
    ['mixrelay-engine-service', 'mixrelay-macos-engine'],
  ]) {
    assert.equal(violation(from, to, 'normal', crates), undefined, `${from} -> ${to}`);
  }
  assert.match(violation('mixrelay-linux-engine', 'mixrelay-macos-transport', 'normal', crates), /not in .* allowed/);
  assert.match(violation('mixrelay-macos-engine', 'mixrelay-control-server', 'normal', crates), /not in .* allowed/);
  assert.match(violation('mixrelay-engine-core', 'mixrelay-macos-engine', 'normal', crates), /not in .* allowed/);
  for (const from of ['mixrelay-macos-transport', 'mixrelay-macos-hal', 'mixrelay-macos-process-tap', 'mixrelay-macos-driver', 'mixrelay-linux-pipewire', 'mixrelay-linux-driver']) {
    assert.match(violation(from, 'mixrelay-engine-core', 'normal'), /not in .* allowed/, from);
  }
});

test('test-* features of workspace crates are enabled only from dev-dependencies', () => {
  const found = violations(metadata([
    ['mixrelay-engine-core', 'mixrelay-time', 'dev', ['test-util']],
    ['mixrelay-macos-hal', 'mixrelay-macos-transport', 'dev', ['test-observation']],
    ['mixrelay-engine-core', 'mixrelay-time', null, ['test-util']],
    ['mixrelay-macos-hal', 'mixrelay-macos-transport', 'build', ['test-observation']],
    // External crates own their feature policy.
    ['mixrelay-desktop-shell', 'tokio', null, ['test-util']],
  ]));
  assert.deepEqual(found, [
    'mixrelay-engine-core -> mixrelay-time (normal): test feature test-util is for dev-dependencies only',
    'mixrelay-macos-hal -> mixrelay-macos-transport (build): test feature test-observation is for dev-dependencies only',
  ]);
});

test('a crate feature table reaches test features only from test-* features', () => {
  const members = new Set(['mixrelay-engine-core', 'mixrelay-time', 'mixrelay-linux-pipewire']);
  const item = (features) => ({
    name: 'mixrelay-engine-core',
    dependencies: [
      { name: 'mixrelay-time', kind: null, features: [] },
      { name: 'mixrelay-linux-pipewire', kind: null, rename: 'pipewire-adapter', features: [] },
      { name: 'tokio', kind: null, features: [] },
    ],
    features,
  });
  // A crate's own test feature may forward to a dependency's, and nothing reaches it from default.
  assert.deepEqual(
    packageFeatureViolations(item({ default: [], 'test-util': ['mixrelay-time/test-util'], 'native-lab': ['tokio/test-util'] }), members),
    [],
  );
  assert.deepEqual(
    packageFeatureViolations(
      item({
        default: ['lab'],
        lab: ['mixrelay-time?/test-util', 'pipewire-adapter/test-observation'],
        'native-lab': ['mixrelay-time/test-util', 'dep:mixrelay-time'],
      }),
      members,
    ),
    [
      'mixrelay-engine-core: feature lab enables mixrelay-time?/test-util; test features are for dev-dependencies only',
      'mixrelay-engine-core: feature lab enables pipewire-adapter/test-observation; test features are for dev-dependencies only',
      'mixrelay-engine-core: feature native-lab enables mixrelay-time/test-util; test features are for dev-dependencies only',
    ],
  );
  assert.deepEqual(packageFeatureViolations(item({ default: ['test-util'], 'test-util': [] }), members), [
    'mixrelay-engine-core: default enables its own test-util; test features are for dev-dependencies only',
  ]);
  assert.deepEqual(
    packageFeatureViolations(item({ default: ['lab'], lab: ['test-util'], 'test-util': [] }), members),
    ['mixrelay-engine-core: default enables its own test-util; test features are for dev-dependencies only'],
  );
  // The workspace check reports the same findings.
  const found = violations({
    packages: [...Object.keys(LAYERS)].map((name) => ({
      name,
      dependencies: name === 'mixrelay-engine-core' ? [{ name: 'mixrelay-time', kind: null, features: [] }] : [],
      features: name === 'mixrelay-engine-core' ? { default: ['mixrelay-time/test-util'] } : {},
    })),
  });
  assert.deepEqual(found, [
    'mixrelay-engine-core: feature default enables mixrelay-time/test-util; test features are for dev-dependencies only',
  ]);
});

test('Linux native fixtures may use the Driver client only as a dev dependency', () => {
  assert.equal(violation('mixrelay-linux-engine', 'mixrelay-driver-adapters', 'dev'), undefined);
  for (const kind of ['normal', 'build']) {
    assert.match(violation('mixrelay-linux-engine', 'mixrelay-driver-adapters', kind), /not in .* allowed/);
  }
});
