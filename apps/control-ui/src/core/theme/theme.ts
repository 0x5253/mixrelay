import { mixrelayVariables, themeRoles } from './tokens';

const declarations = (variables: Record<string, string>) =>
  Object.entries(variables)
    .map(([name, value]) => `${name}:${value}`)
    .join(';');

// Regenerated from tokens on each build: no second, hand-maintained color definition.
export const themeCss = `
:root{${declarations(mixrelayVariables)};${declarations(themeRoles.light)};color-scheme:light}
[lang]{${declarations({ '--font-sans': mixrelayVariables['--font-sans'], '--font-mono': mixrelayVariables['--font-mono'] })}}
:root[data-theme="dark"]{${declarations(themeRoles.dark)};color-scheme:dark}
@media(prefers-color-scheme:dark){:root:not([data-theme="light"]){${declarations(themeRoles.dark)};color-scheme:dark}}
`;
