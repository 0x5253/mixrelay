import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

function suiteOnly(path) {
  return path.startsWith('docs/') || path.startsWith('scripts/e2e/') ||
    ['.github/workflows/candidate-e2e.yml', '.github/workflows/qualification-hosts.yml',
      '.github/workflows/installed-evidence.yml'].includes(path);
}

export function compareSuiteSource({ sourceRevision, suiteRevision, sourceTree, suiteTree }) {
  assert.match(sourceRevision, /^[a-f0-9]{40}$/);
  assert.match(suiteRevision, /^[a-f0-9]{40}$/);
  assert.equal(sourceTree.truncated, false, 'Installer source tree is incomplete');
  const source = new Map(sourceTree.tree.filter(entry => entry.type !== 'tree')
    .map(({ path, mode, sha }) => [path, `${mode} ${sha}`]));
  const suite = new Map(suiteTree.split('\0').filter(Boolean).map(entry => {
    const separator = entry.indexOf('\t');
    assert.ok(separator > 0, 'Invalid suite tree entry');
    const [mode, , sha] = entry.slice(0, separator).split(' ');
    return [entry.slice(separator + 1), `${mode} ${sha}`];
  }));
  const changed = [...new Set([...source.keys(), ...suite.keys()])]
    .filter(path => source.get(path) !== suite.get(path)).sort();
  const productChanges = changed.filter(path => !suiteOnly(path));
  return { sourceRevision, suiteRevision, sourceTreeSha: sourceTree.sha,
    changedSuiteFiles: changed.filter(suiteOnly), productChanges,
    productSourceMatches: productChanges.length === 0 };
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const [candidateFile, treeFile, output] = process.argv.slice(2);
  assert.ok(candidateFile && treeFile && output, 'Usage: suite-source.mjs CANDIDATE SOURCE_TREE OUTPUT');
  const sourceRevision = JSON.parse(readFileSync(candidateFile)).sourceRevision;
  const suiteRevision = execFileSync('git', ['rev-parse', 'HEAD'], { encoding: 'utf8' }).trim();
  assert.equal(suiteRevision, process.env.GITHUB_SHA, 'Checkout does not match the suite commit');
  const comparison = compareSuiteSource({ sourceRevision, suiteRevision,
    sourceTree: JSON.parse(readFileSync(treeFile)),
    suiteTree: execFileSync('git', ['ls-tree', '-r', '-z', 'HEAD'], { encoding: 'utf8', maxBuffer: 16 * 1024 * 1024 }),
  });
  writeFileSync(output, JSON.stringify(comparison, null, 2) + '\n', { flag: 'wx' });
  assert.equal(comparison.productSourceMatches, true,
    `Product or packaging changed since the installers were built: ${comparison.productChanges.join(', ')}`);
}
