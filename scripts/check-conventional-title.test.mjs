import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { titleProblem } from './check-conventional-title.mjs';

const script = fileURLToPath(new URL('./check-conventional-title.mjs', import.meta.url));

test('accepts conventional change titles', () => {
  for (const title of ['fix: keep PCM bounds', 'feat(latency): report host spans', 'refactor(desktop)!: drop the old socket',
    'feat!: change the graph format', 'docs(website): add the manual', 'chore(main): release 0.2.0', 'fix(ui,audio): align meters',
    'test(engine-process): cover restart', 'revert: feat(latency): report host spans']) {
    assert.equal(titleProblem(title), undefined, title);
  }
});

test('rejects titles without a conventional change type', () => {
  for (const title of ['Update README', 'Merge pull request #2 from mixrelay/feat', 'feature: add recorder', 'Fix: capital type',
    'fix:missing space', 'fix: ', 'fix(): empty scope', 'fix(UI): uppercase scope', 'wip']) {
    assert.match(titleProblem(title) ?? '', /not a Conventional Commits title/, title);
  }
});

test('the command fails when any given title is invalid', () => {
  assert.doesNotThrow(() => execFileSync(process.execPath, [script, 'fix: one', 'feat(audio): two'], { stdio: 'pipe' }));
  assert.throws(() => execFileSync(process.execPath, [script, 'fix: one', 'Second title'], { stdio: 'pipe' }),
    error => error.status === 1 && /"Second title" is not/.test(error.stderr));
  assert.throws(() => execFileSync(process.execPath, [script], { stdio: 'pipe' }), error => error.status === 1);
});
