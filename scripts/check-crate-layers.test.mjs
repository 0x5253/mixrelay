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
    ['mixrelay-audio-engine', 'mixrelay-audio-graph'],
    ['mixrelay-control-protocol', 'mixrelay-audio-graph'],
    ['mixrelay-control-server', 'mixrelay-control-protocol'],
    ['mixrelay-processor-host', 'mixrelay-control-protocol'],
    ['mixrelay-processor-host', 'truce-rack-vst3'],
    ['mixrelay-hal-core', 'mixrelay-macos-transport'],
    ['mixrelay-hal-core', 'mixrelay-pcm-ring'],
    ['mixrelay-macos-driver', 'mixrelay-driver-control'],
    ['mixrelay-linux-driver', 'mixrelay-driver-control'],
    ['mixrelay-linux-driver', 'mixrelay-pipewire'],
    ['mixrelay-service', 'mixrelay-pipewire'],
    ['mixrelay-service', 'truce-rack-core'],
    ['mixrelay-service', 'mixrelay-engine'],
    ['mixrelay-audio-engine', 'mixrelay-time'],
    ['mixrelay-engine', 'mixrelay-time'],
    ['mixrelay-engine', 'mixrelay-audio-graph'],
    ['mixrelay-engine', 'mixrelay-control-protocol'],
    ['mixrelay-engine', 'mixrelay-driver-control'],
    ['mixrelay-driver-adapters', 'mixrelay-driver-control'],
    ['mixrelay-macos-installer', 'mixrelay-driver-adapters'],
  ]) {
    assert.equal(violation(from, to, 'normal'), undefined, `${from} -> ${to}`);
  }
});

test('each crate gets exactly its row: no blanket layer allowances', () => {
  // Adapters with no contract in their row may not use one, even though other adapters do.
  for (const from of ['mixrelay-macos-transport', 'mixrelay-hal-core', 'mixrelay-process-tap', 'mixrelay-pipewire']) {
    assert.match(violation(from, 'mixrelay-driver-control', 'normal'), /not in .* allowed/, from);
  }
  assert.match(violation('mixrelay-macos-installer', 'mixrelay-macos-transport', 'normal'), /not in .* allowed/);
  assert.match(violation('mixrelay-hal-bundle', 'mixrelay-audio-io', 'normal'), /not in .* allowed/);
  assert.match(violation('mixrelay-driver-control', 'mixrelay-driver-adapters', 'normal'), /not in .* allowed/);
  assert.match(violation('mixrelay-engine', 'mixrelay-driver-adapters', 'normal'), /not in .* allowed/);
  assert.match(violation('mixrelay-driver-adapters', 'mixrelay-engine', 'normal'), /not in .* allowed/);
  assert.match(violation('mixrelay-driver-control', 'mixrelay-audio-graph', 'normal'), /not in .* allowed/);
  assert.match(violation('mixrelay-audio-graph', 'mixrelay-audio-recorder', 'normal'), /not in .* allowed/);
  // The engine reaches recording only through its own sink port.
  assert.match(violation('mixrelay-audio-engine', 'mixrelay-audio-recorder', 'normal'), /not in .* allowed/);
  assert.match(violation('mixrelay-control-protocol', 'mixrelay-driver-control', 'normal'), /not in .* allowed/);
  // Build dependencies follow the same row.
  assert.match(violation('mixrelay-audio-io', 'mixrelay-realtime', 'build'), /not in .* allowed/);
});

test('upward, cross-platform, app and test-support dependencies are rejected', () => {
  assert.match(violation('mixrelay-audio-io', 'mixrelay-audio-graph', 'normal'), /not in .* allowed/);
  assert.match(violation('mixrelay-audio-engine', 'mixrelay-macos-transport', 'normal'), /not in .* allowed/);
  assert.match(violation('mixrelay-pipewire', 'mixrelay-macos-transport', 'normal'), /not in .* allowed/);
  assert.match(violation('mixrelay-control-server', 'mixrelay-service', 'normal'), /app/);
  assert.match(violation('mixrelay-desktop', 'mixrelay-service', 'normal'), /app/);
  assert.match(violation('mixrelay-audio-io', 'mixrelay-test-support', 'normal'), /dev-dependency/);
  assert.match(violation('mixrelay-service', 'mixrelay-macos-test-support', 'normal'), /dev-dependency/);
  assert.equal(violation('mixrelay-audio-io', 'mixrelay-test-support', 'dev'), undefined);
  assert.match(violation('mixrelay-audio-engine', 'truce-rack-core', 'normal'), /not in .* allowed/);
  assert.match(violation('mixrelay-control-server', 'truce-rack-core', 'normal'), /not in .* allowed/);
  assert.match(violation('mixrelay-new', 'mixrelay-audio-io', 'normal'), /no row/);
  assert.match(violation('mixrelay-audio-io', 'mixrelay-new', 'normal'), /no row/);
  // Dev dependencies follow the same table; only test support is added.
  assert.match(violation('mixrelay-audio-io', 'mixrelay-desktop', 'dev'), /app/);
  assert.match(violation('mixrelay-audio-graph', 'mixrelay-pipewire', 'dev'), /not in .* allowed/);
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
    ['mixrelay-audio-io', 'mixrelay-audio-engine'],
    ['mixrelay-audio-io', 'mixrelay-audio-engine', 'build'],
    ['mixrelay-audio-graph', 'mixrelay-audio-io'],
    ['mixrelay-hal-core', 'mixrelay-driver-control', 'dev'],
  ]));
  assert.deepEqual(found, [
    "mixrelay-audio-io -> mixrelay-audio-engine (build): not in mixrelay-audio-io's allowed dependencies in the crate dependency table",
    "mixrelay-audio-io -> mixrelay-audio-engine (normal): not in mixrelay-audio-io's allowed dependencies in the crate dependency table",
    "mixrelay-hal-core -> mixrelay-driver-control (dev): not in mixrelay-hal-core's allowed dependencies in the crate dependency table",
  ]);
});

test('allow-lists only name registered crates', () => {
  for (const [name, row] of Object.entries(CRATES)) {
    if (!Array.isArray(row.allows)) continue;
    for (const to of row.allows) assert.ok(CRATES[to], `${name} allows unregistered ${to}`);
  }
});

test('the engine domain sits above model and contract crates, beside service parts and below adapters and apps', () => {
  for (const to of ['mixrelay-macos-transport', 'mixrelay-pipewire', 'truce-rack-core']) {
    assert.match(violation('mixrelay-engine', to, 'normal'), /not in .* allowed/, to);
  }
  assert.match(violation('mixrelay-engine', 'mixrelay-service', 'normal'), /app/);
  // Inner crates define the ports they use instead of reaching up to the domain crate.
  for (const from of ['mixrelay-time', 'mixrelay-audio-engine', 'mixrelay-control-protocol']) {
    assert.match(violation(from, 'mixrelay-engine', 'normal'), /not in .* allowed/, from);
  }
  // The domain crate and the service parts are peers: neither depends on the other.
  for (const part of ['mixrelay-control-server', 'mixrelay-processor-host']) {
    assert.match(violation('mixrelay-engine', part, 'normal'), /not in .* allowed/, part);
    assert.match(violation(part, 'mixrelay-engine', 'normal'), /not in .* allowed/, part);
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
  const linuxEngine = { layer: 'linux', allows: engineAdapter('mixrelay-pipewire') };
  assert.ok(linuxEngine.allows.every((to) => CRATES[to].layer !== 'macos'));
});

test('engine adapters may use mixrelay-engine; other adapters may not', () => {
  // No engine adapter exists yet, so check rows shaped like the ones that will be added.
  const crates = {
    ...CRATES,
    'mixrelay-macos-engine': { layer: 'macos', allows: engineAdapter('mixrelay-macos-transport', 'mixrelay-process-tap') },
    'mixrelay-linux-engine': { layer: 'linux', allows: engineAdapter('mixrelay-pipewire') },
  };
  for (const [from, to] of [
    ['mixrelay-macos-engine', 'mixrelay-engine'],
    ['mixrelay-macos-engine', 'mixrelay-time'],
    ['mixrelay-macos-engine', 'mixrelay-audio-graph'],
    ['mixrelay-macos-engine', 'mixrelay-driver-control'],
    ['mixrelay-macos-engine', 'mixrelay-process-tap'],
    ['mixrelay-linux-engine', 'mixrelay-engine'],
    ['mixrelay-linux-engine', 'mixrelay-pipewire'],
    ['mixrelay-service', 'mixrelay-macos-engine'],
  ]) {
    assert.equal(violation(from, to, 'normal', crates), undefined, `${from} -> ${to}`);
  }
  assert.match(violation('mixrelay-linux-engine', 'mixrelay-macos-transport', 'normal', crates), /not in .* allowed/);
  assert.match(violation('mixrelay-macos-engine', 'mixrelay-control-server', 'normal', crates), /not in .* allowed/);
  assert.match(violation('mixrelay-engine', 'mixrelay-macos-engine', 'normal', crates), /not in .* allowed/);
  for (const from of ['mixrelay-macos-transport', 'mixrelay-hal-core', 'mixrelay-process-tap', 'mixrelay-macos-driver', 'mixrelay-pipewire', 'mixrelay-linux-driver']) {
    assert.match(violation(from, 'mixrelay-engine', 'normal'), /not in .* allowed/, from);
  }
});

test('test-* features of workspace crates are enabled only from dev-dependencies', () => {
  const found = violations(metadata([
    ['mixrelay-engine', 'mixrelay-time', 'dev', ['test-util']],
    ['mixrelay-hal-core', 'mixrelay-macos-transport', 'dev', ['test-observation']],
    ['mixrelay-engine', 'mixrelay-time', null, ['test-util']],
    ['mixrelay-hal-core', 'mixrelay-macos-transport', 'build', ['test-observation']],
    // External crates own their feature policy.
    ['mixrelay-desktop', 'tokio', null, ['test-util']],
  ]));
  assert.deepEqual(found, [
    'mixrelay-engine -> mixrelay-time (normal): test feature test-util is for dev-dependencies only',
    'mixrelay-hal-core -> mixrelay-macos-transport (build): test feature test-observation is for dev-dependencies only',
  ]);
});

test('a crate feature table reaches test features only from test-* features', () => {
  const members = new Set(['mixrelay-engine', 'mixrelay-time', 'mixrelay-pipewire']);
  const item = (features) => ({
    name: 'mixrelay-engine',
    dependencies: [
      { name: 'mixrelay-time', kind: null, features: [] },
      { name: 'mixrelay-pipewire', kind: null, rename: 'pipewire-adapter', features: [] },
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
      'mixrelay-engine: feature lab enables mixrelay-time?/test-util; test features are for dev-dependencies only',
      'mixrelay-engine: feature lab enables pipewire-adapter/test-observation; test features are for dev-dependencies only',
      'mixrelay-engine: feature native-lab enables mixrelay-time/test-util; test features are for dev-dependencies only',
    ],
  );
  assert.deepEqual(packageFeatureViolations(item({ default: ['test-util'], 'test-util': [] }), members), [
    'mixrelay-engine: default enables its own test-util; test features are for dev-dependencies only',
  ]);
  assert.deepEqual(
    packageFeatureViolations(item({ default: ['lab'], lab: ['test-util'], 'test-util': [] }), members),
    ['mixrelay-engine: default enables its own test-util; test features are for dev-dependencies only'],
  );
  // The workspace check reports the same findings.
  const found = violations({
    packages: [...Object.keys(LAYERS)].map((name) => ({
      name,
      dependencies: name === 'mixrelay-engine' ? [{ name: 'mixrelay-time', kind: null, features: [] }] : [],
      features: name === 'mixrelay-engine' ? { default: ['mixrelay-time/test-util'] } : {},
    })),
  });
  assert.deepEqual(found, [
    'mixrelay-engine: feature default enables mixrelay-time/test-util; test features are for dev-dependencies only',
  ]);
});

test('Linux native fixtures may use the Driver client only as a dev dependency', () => {
  assert.equal(violation('mixrelay-linux-engine', 'mixrelay-driver-adapters', 'dev'), undefined);
  for (const kind of ['normal', 'build']) {
    assert.match(violation('mixrelay-linux-engine', 'mixrelay-driver-adapters', kind), /not in .* allowed/);
  }
});
