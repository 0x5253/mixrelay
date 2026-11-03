#!/usr/bin/env node
import { PRODUCT_JSON_MANIFESTS, HAL_MANIFEST, productVersionFrom, workspaceMembersFrom, productCratesFrom, checkVersionSnapshot, planVersion } from './core/product-version-plan.mjs';
import { readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const root = fileURLToPath(new URL('..', import.meta.url));
function sources(directory) {
  const cargo = readFileSync(resolve(directory, 'Cargo.toml'), 'utf8');
  const names = ['Cargo.toml', 'Cargo.lock', HAL_MANIFEST, ...PRODUCT_JSON_MANIFESTS,
    ...workspaceMembersFrom(cargo).map(member => `${member}/Cargo.toml`)];
  return new Map(names.map(name => [name, readFileSync(resolve(directory, name), 'utf8')]));
}
export function productVersion(directory = root) {
  return productVersionFrom(readFileSync(resolve(directory, 'Cargo.toml'), 'utf8'));
}
export function workspaceMembers(directory = root) {
  return workspaceMembersFrom(readFileSync(resolve(directory, 'Cargo.toml'), 'utf8'));
}
export function productCrates(directory = root) {
  const cargo = readFileSync(resolve(directory, 'Cargo.toml'), 'utf8');
  return productCratesFrom(new Map([['Cargo.toml', cargo], ...workspaceMembersFrom(cargo)
    .map(member => [`${member}/Cargo.toml`, readFileSync(resolve(directory, member, 'Cargo.toml'), 'utf8')])]));
}
export function checkVersion(directory = root) { return checkVersionSnapshot(sources(directory)); }
export function setVersion(version, directory = root) {
  const edits = planVersion(sources(directory), version);
  for (const [name, text] of edits) writeFileSync(resolve(directory, name), text);
  return checkVersion(directory);
}
if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  try {
    const [action = 'check', version, ...extra] = process.argv.slice(2);
    if (action === '--help') console.log('pnpm product:version [check | set MAJOR.MINOR.PATCH]\nCargo workspace is the product version; app and all native components ship together.\nset also updates the Cargo.lock entries of workspace crates that inherit it, so `cargo build --locked` stays valid.');
    else if (action === 'check' && version === undefined) console.log(`MixRelay ${checkVersion()}`);
    else if (action === 'set' && version && extra.length === 0) console.log(`MixRelay ${setVersion(version)}`);
    else throw new Error('Use --help for product version commands');
  } catch (error) { console.error(error.message); process.exitCode = 1; }
}
