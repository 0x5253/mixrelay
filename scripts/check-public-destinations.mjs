import { assertPublicDestination, extractDestinations, assertPublicReferences, isUserFacingSurface, publicSurfaceText } from './core/public-references.mjs';
/**
 * Public destination contract. Scope: root README; shipped licence/legal texts
 * (including bundled third-party notices); control-ui production TS/TSX/CSS and
 * locales; desktop/native production sources (menus/dialogs included); installer
 * resources and generators; Linux recipes/desktop entries; Cargo/npm/Tauri package
 * metadata; Stream Deck manifest, runtime and property inspector; website HTML,
 * CSS, JS, JSON and legal text; checked-in update/release manifests and generated
 * release manifests before writing. Arch metadata is inspected after substitution.
 * Excludes tests/fixtures (including inline Rust test modules), Storybook/stories,
 * developer docs, build output, dependency source, CI/qualification evidence,
 * font provenance, local development/Stream Deck transports, XML namespaces/DTDs
 * and JSON schema identifiers.
 * This is an offline allowlist, not a network availability or mailbox delivery test.
 */
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { renderArchRecipe } from './core/arch-recipe.mjs';
import { productContact } from './core/product-metadata-policy.mjs';

const root = fileURLToPath(new URL('..', import.meta.url));
export function checkPublicDestinations(directory = root) {
  const deleted = new Set(execFileSync('git', ['ls-files', '--deleted', '-z'], { cwd: directory, encoding: 'utf8' }).split('\0'));
  const files = execFileSync('git', ['ls-files', '--cached', '--others', '--exclude-standard', '-z'], { cwd: directory, encoding: 'utf8' })
    .split('\0').filter(Boolean).filter(path => !deleted.has(path)).filter(isUserFacingSurface);
  for (const path of files) assertPublicReferences(publicSurfaceText(readFileSync(resolve(directory, path), 'utf8'), path), path);
  const contract = JSON.parse(readFileSync(resolve(directory, 'support/environments.json')));
  assertPublicReferences(JSON.stringify(contract.metadata), 'support product metadata');
  assertPublicReferences(renderArchRecipe({ version: '0.1.0', architecture: 'amd64', checksum: 'a'.repeat(64) }, { template: readFileSync(resolve(directory, 'packaging/linux/arch/PKGBUILD.in'), 'utf8'),
    metadata: { ...contract.metadata, contact: productContact(contract.metadata) }, minimums: contract.minimums }), 'rendered Arch metadata');
  return files.length;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try { console.log(`Public destinations passed (${checkPublicDestinations()} surfaces).`); }
  catch (error) { console.error(error.message); process.exitCode = 1; }
}
