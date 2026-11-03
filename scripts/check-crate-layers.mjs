#!/usr/bin/env node
// Checks that workspace crates depend only on what the crate dependency table allows.
// Reads `cargo metadata`; prints each forbidden dependency and each workspace crate missing
// from the table, and exits 1 when any exist. Normal, build and dev dependencies all follow
// the table; dev dependencies may additionally use test support and enable a workspace
// crate's `test-*` features (test fakes and observation hooks). In a crate's own
// `[features]`, only `test-*` features may forward to them, and `default` never reaches one.
import { execFileSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';

const FOUNDATION = [
  'mixrelay-audio-io',
  'mixrelay-pcm-ring',
  'mixrelay-realtime',
  'mixrelay-time',
  'mixrelay-local-socket',
  'mixrelay-processor-state',
  'mixrelay-engine-process',
  'mixrelay-midi-control',
];
const MODELS = ['mixrelay-audio-graph', 'mixrelay-audio-recorder', 'mixrelay-audio-engine'];
const CONTRACTS = ['mixrelay-control-protocol', 'mixrelay-driver-control'];
const DOMAIN = ['mixrelay-engine'];
const PLUGIN_FORMATS = ['truce-rack-core', 'truce-rack-clap', 'truce-rack-vst3'];
/** Every non-app crate; apps may use any of them ("앱 외 전부"). Test support stays dev-only. */
const ALL_BUT_APPS = Symbol('all but apps');

/**
 * Allowances of an engine adapter row: an OS crate under native/ that implements
 * `mixrelay-engine` ports. It may use the domain crate and the foundation, model and contract
 * types its ports carry, plus the same-OS adapters it names. Other adapters may not use the
 * domain crate. Engine adapter rows list the same-OS implementations they compose.
 */
export function engineAdapter(...sameOsAdapters) {
  return [...FOUNDATION, ...MODELS, ...CONTRACTS, ...DOMAIN, ...sameOsAdapters];
}

/**
 * One row per crate, with its allowed workspace dependencies.
 * devAllows grants only the named test dependency; product edges remain forbidden.
 */
export const CRATES = {
  'mixrelay-midi-control': { layer: 'foundation', allows: [] },
  'mixrelay-audio-io': { layer: 'foundation', allows: [] },
  'mixrelay-pcm-ring': { layer: 'foundation', allows: [] },
  'mixrelay-realtime': { layer: 'foundation', allows: [] },
  'mixrelay-local-socket': { layer: 'foundation', allows: [] },
  'mixrelay-processor-state': { layer: 'foundation', allows: [] },
  'mixrelay-engine-process': { layer: 'foundation', allows: ['mixrelay-time'] },
  'mixrelay-time': { layer: 'foundation', allows: [] },
  'mixrelay-audio-graph': { layer: 'model', allows: FOUNDATION },
  'mixrelay-audio-recorder': { layer: 'model', allows: FOUNDATION },
  'mixrelay-audio-engine': { layer: 'model', allows: [...FOUNDATION, 'mixrelay-audio-graph'] },
  'mixrelay-control-protocol': { layer: 'contract', allows: [...FOUNDATION, ...MODELS] },
  'mixrelay-driver-control': { layer: 'contract', allows: FOUNDATION },
  'mixrelay-engine': { layer: 'domain', allows: [...FOUNDATION, ...MODELS, ...CONTRACTS] },
  'mixrelay-engine-runtime': { layer: 'native', allows: engineAdapter() },
  'mixrelay-desktop-application': { layer: 'domain', allows: ['mixrelay-time', 'mixrelay-audio-io', 'mixrelay-control-protocol', 'mixrelay-driver-control', 'mixrelay-audio-graph', 'mixrelay-midi-control'] },
  'mixrelay-macos-audio-io': { layer: 'macos', allows: ['mixrelay-audio-io'] },
  'mixrelay-macos-engine': { layer: 'macos', allows: engineAdapter('mixrelay-engine-runtime', 'mixrelay-macos-transport', 'mixrelay-macos-audio-io', 'mixrelay-process-tap') },
  'mixrelay-linux-engine': { layer: 'linux', allows: engineAdapter('mixrelay-engine-runtime', 'mixrelay-linux-driver', 'mixrelay-linux-audio-io', 'mixrelay-pipewire'), devAllows: ['mixrelay-driver-adapters'] },
  'mixrelay-linux-audio-io': { layer: 'linux', allows: ['mixrelay-audio-io', 'mixrelay-time'] },
  'mixrelay-driver-adapters': { layer: 'service', allows: [...FOUNDATION, 'mixrelay-driver-control'] },
  'mixrelay-control-server': { layer: 'service', allows: [...FOUNDATION, ...MODELS, ...CONTRACTS] },
  // The table's known exception: plugin descriptors and editor frames live in the protocol.
  'mixrelay-processor-host': { layer: 'service', allows: [...FOUNDATION, ...MODELS, 'mixrelay-control-protocol', ...PLUGIN_FORMATS] },
  'mixrelay-macos-transport': { layer: 'macos', allows: FOUNDATION },
  'mixrelay-hal-core': { layer: 'macos', allows: [...FOUNDATION, 'mixrelay-macos-transport'] },
  'mixrelay-process-tap': { layer: 'macos', allows: [...FOUNDATION, 'mixrelay-macos-transport'] },
  'mixrelay-macos-driver': { layer: 'macos', allows: [...FOUNDATION, ...CONTRACTS, 'mixrelay-macos-transport'] },
  'mixrelay-macos-installer': { layer: 'macos', allows: [...FOUNDATION, ...CONTRACTS, 'mixrelay-driver-adapters'] },
  'mixrelay-hal-bundle': { layer: 'macos', allows: [] },
  'mixrelay-pipewire': { layer: 'linux', allows: FOUNDATION },
  'mixrelay-linux-driver': { layer: 'linux', allows: [...FOUNDATION, ...CONTRACTS, 'mixrelay-pipewire', 'mixrelay-linux-audio-io'] },
  'mixrelay-service': { layer: 'app', allows: ALL_BUT_APPS },
  'mixrelay-driver-service': { layer: 'app', allows: ALL_BUT_APPS },
  'mixrelay-desktop': { layer: 'app', allows: ALL_BUT_APPS },
  'mixrelay-test-support': { layer: 'test', allows: [] },
  'mixrelay-macos-test-support': { layer: 'test', allows: [] },
  // tests/fixtures/*
  'mixrelay-native-clap-fixture': { layer: 'test', allows: [] },
  // A diagnostic preload with no workspace deps.
  'mixrelay-pipewire-observer': { layer: 'test', allows: [] },
  // Vendored plugin formats.
  'truce-rack-core': { layer: 'vendor', allows: [] },
  'truce-rack-clap': { layer: 'vendor', allows: ['truce-rack-core'] },
  'truce-rack-vst3': { layer: 'vendor', allows: ['truce-rack-core'] },
};

/** Layer of each registered crate. */
export const LAYERS = Object.fromEntries(Object.entries(CRATES).map(([name, row]) => [name, row.layer]));

function allowed(from, to, crates) {
  const { allows } = crates[from];
  if (allows === ALL_BUT_APPS) return !['app', 'test'].includes(crates[to].layer);
  return allows.includes(to);
}

/**
 * Why `from -> to` is forbidden, or undefined when the table allows it. Tests pass `crates`
 * to check rows that are not in the workspace yet.
 */
export function violation(from, to, kind, crates = CRATES) {
  if (crates[from] === undefined) return `${from} has no row in scripts/check-crate-layers.mjs (crate dependency table)`;
  if (crates[to] === undefined) return `${to} has no row in scripts/check-crate-layers.mjs (crate dependency table)`;
  if (crates[to].layer === 'test') return kind === 'dev' ? undefined : 'test support is a dev-dependency only';
  if (crates[to].layer === 'app') return 'nothing depends on an app';
  if (kind === 'dev' && crates[from].devAllows?.includes(to)) return undefined;
  if (allowed(from, to, crates)) return undefined;
  return `not in ${from}'s allowed dependencies in the crate dependency table`;
}

const isTestFeature = (feature) => feature.startsWith('test-');

/** Why a `kind` dependency on a workspace crate may not enable `features`, or undefined. */
export function featureViolation(kind, features) {
  const test = features.filter(isTestFeature);
  if (kind === 'dev' || test.length === 0) return undefined;
  return `test feature ${test.join(', ')} is for dev-dependencies only`;
}

/**
 * Ways a package's own `[features]` table turns a test feature on outside dev-dependencies:
 * a feature not named `test-*` (including `default`) that forwards to a workspace
 * dependency's `test-*` feature, or a `default` that reaches the package's own `test-*`
 * feature directly or through other features.
 */
export function packageFeatureViolations(item, members) {
  const features = item.features ?? {};
  const workspaceKeys = new Set(
    item.dependencies.filter((dependency) => members.has(dependency.name)).map((dependency) => dependency.rename ?? dependency.name),
  );
  const found = [];
  for (const [feature, entries] of Object.entries(features)) {
    if (isTestFeature(feature)) continue;
    for (const entry of entries) {
      const forward = entry.match(/^([^/?]+)\??\/(.+)$/);
      if (forward && workspaceKeys.has(forward[1]) && isTestFeature(forward[2])) {
        found.push(`${item.name}: feature ${feature} enables ${entry}; test features are for dev-dependencies only`);
      }
    }
  }
  const reached = new Set();
  const pending = ['default'];
  while (pending.length > 0) {
    const feature = pending.pop();
    if (reached.has(feature) || features[feature] === undefined) continue;
    reached.add(feature);
    if (isTestFeature(feature)) {
      found.push(`${item.name}: default enables its own ${feature}; test features are for dev-dependencies only`);
      continue;
    }
    pending.push(...features[feature].filter((entry) => !entry.includes('/') && !entry.startsWith('dep:')));
  }
  return found;
}

/** Forbidden dependencies and unregistered or stale crates in `cargo metadata` output. */
export function violations(metadata) {
  const members = new Set(metadata.packages.map((item) => item.name));
  const found = [];
  for (const item of metadata.packages) {
    if (CRATES[item.name] === undefined) {
      found.push(`${item.name}: workspace crate has no row in the crate dependency table and scripts/check-crate-layers.mjs`);
    }
    for (const dependency of item.dependencies) {
      if (!members.has(dependency.name)) continue;
      const kind = dependency.kind === 'dev' ? 'dev' : dependency.kind === 'build' ? 'build' : 'normal';
      const reason = violation(item.name, dependency.name, kind);
      if (reason) found.push(`${item.name} -> ${dependency.name} (${kind}): ${reason}`);
      const featureReason = featureViolation(kind, dependency.features ?? []);
      if (featureReason) found.push(`${item.name} -> ${dependency.name} (${kind}): ${featureReason}`);
    }
    found.push(...packageFeatureViolations(item, members));
  }
  for (const name of Object.keys(CRATES)) {
    if (!members.has(name)) found.push(`${name}: listed in scripts/check-crate-layers.mjs but not a workspace crate`);
  }
  return [...new Set(found)].sort();
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const metadata = JSON.parse(
    execFileSync('cargo', ['metadata', '--format-version', '1', '--no-deps'], {
      encoding: 'utf8',
      maxBuffer: 64 * 1024 * 1024,
    }),
  );
  const found = violations(metadata);
  for (const line of found) console.error(line);
  if (found.length > 0) {
    console.error('Crate dependencies break the crate dependency table.');
    process.exit(1);
  }
  console.log(`Crate layers checked: ${metadata.packages.length} workspace crates.`);
}
