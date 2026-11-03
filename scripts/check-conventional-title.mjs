#!/usr/bin/env node
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

// Conventional titles distinguish fixes, features and breaking changes.
export const TYPES = ['build', 'chore', 'ci', 'docs', 'feat', 'fix', 'perf', 'refactor', 'revert', 'style', 'test'];
const pattern = new RegExp(`^(?:${TYPES.join('|')})(?:\\([a-z0-9][a-z0-9._/,-]*\\))?!?: \\S`);

export function titleProblem(title) {
  if (pattern.test(title)) return undefined;
  return `"${title}" is not a Conventional Commits title: use TYPE(optional-scope)!?: summary with TYPE one of ${TYPES.join(', ')}`;
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  const titles = process.argv.slice(2);
  if (titles.length === 0 || titles[0] === '--help') {
    console.log('node scripts/check-conventional-title.mjs TITLE...\nFails when any TITLE (a PR title or a squash commit subject) is not a Conventional Commits title.');
    process.exitCode = titles.length === 0 ? 1 : 0;
  } else {
    const problems = titles.map(titleProblem).filter(Boolean);
    for (const problem of problems) console.error(problem);
    process.exitCode = problems.length ? 1 : 0;
  }
}
