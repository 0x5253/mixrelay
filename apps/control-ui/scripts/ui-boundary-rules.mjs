import { parse } from '@babel/parser';
import * as css from 'css-tree';

const controls = new Set(['button', 'input', 'select', 'textarea', 'option']);
const stylingProps = new Set([
  'style',
  'styles',
  'classNames',
  'vars',
  'unstyled',
  'renderRoot',
  'dangerouslySetInnerHTML',
]);
const libraryPattern = /^(?:@mantine\/|react-aria-components(?:$|\/)|@xyflow\/react(?:$|\/))/u;
const colorPattern = /#[\da-f]{3,8}\b|\b(?:rgba?|hsla?|hwb|lab|lch|oklab|oklch|color)\(/iu;
const layoutProperties = new Set([
  'display',
  'position',
  'top',
  'right',
  'bottom',
  'left',
  'inset',
  'z-index',
  'width',
  'height',
  'min-width',
  'min-height',
  'max-width',
  'max-height',
  'overflow',
  'overflow-x',
  'overflow-y',
  'overflow-wrap',
  'overscroll-behavior',
  'overscroll-behavior-x',
  'overscroll-behavior-y',
  'box-sizing',
  'visibility',
  'flex',
  'flex-direction',
  'flex-wrap',
  'flex-grow',
  'flex-shrink',
  'flex-basis',
  'align-items',
  'align-self',
  'justify-content',
  'justify-self',
  'place-items',
  'gap',
  'row-gap',
  'column-gap',
  'order',
  'grid',
  'grid-area',
  'grid-column',
  'grid-row',
  'grid-template-columns',
  'grid-template-rows',
  'grid-auto-flow',
  'grid-auto-rows',
  'margin',
  'margin-top',
  'margin-right',
  'margin-bottom',
  'margin-left',
  'margin-inline',
  'padding',
  'padding-top',
  'padding-right',
  'padding-bottom',
  'padding-left',
  'padding-inline',
]);

function visit(node, callback) {
  if (!node || typeof node !== 'object') return;
  if (typeof node.type === 'string' && callback(node) === false) return;
  const children = Object.entries(node)
    .filter(([key]) => !['loc', 'comments', 'tokens'].includes(key))
    .flatMap(([, value]) => (Array.isArray(value) ? value : [value]));
  for (const child of children) visit(child, callback);
}

function issue(violations, node, code, message) {
  violations.push({ line: node.loc?.start?.line ?? 1, code, message });
}

function checkLibrary(node, core, violations) {
  const source =
    node.source?.value ?? (node.type === 'CallExpression' ? node.arguments[0]?.value : undefined);
  if (!core && typeof source === 'string' && libraryPattern.test(source)) {
    issue(violations, node, 'library', 'UI library imports belong in Core.');
  }
  if (!core && node.type === 'ImportDeclaration' && /\/core\/.*\.css$/u.test(source)) {
    issue(
      violations,
      node,
      'selector',
      'Consume the Core component instead of importing its stylesheet.',
    );
  }
  if (
    !core &&
    node.type === 'ImportDeclaration' &&
    /\.css$/u.test(source) &&
    !source.endsWith('.module.css')
  ) {
    issue(violations, node, 'selector', 'Global styles belong in the Core theme.');
  }
}

function inlineLayout(attribute) {
  const expression = attribute.value?.expression;
  return (
    expression?.type === 'ObjectExpression' &&
    expression.properties.every((property) => {
      const name = property.key?.name ?? property.key?.value;
      const dashed = name?.replace(/[A-Z]/gu, (letter) => `-${letter.toLowerCase()}`);
      return (
        property.type === 'ObjectProperty' && !property.computed && layoutProperties.has(dashed)
      );
    })
  );
}

function expandedAttributes(attributes, bindings, seen = new Set()) {
  const result = [];
  for (const attribute of attributes) {
    if (attribute.type === 'JSXAttribute') {
      result.push(attribute);
      continue;
    }
    const argument = attribute.argument;
    if (argument?.type === 'Identifier' && !seen.has(argument.name)) {
      const next = new Set([...seen, argument.name]);
      result.push(
        ...expandedAttributes([{ argument: bindings.get(argument.name) }], bindings, next),
      );
    }
    if (argument?.type === 'ObjectExpression') {
      const members = argument.properties.map((property) =>
        property.type === 'ObjectProperty'
          ? {
              type: 'JSXAttribute',
              name: { name: property.key.name ?? property.key.value },
              value: { expression: property.value },
              loc: property.loc,
            }
          : property,
      );
      result.push(...expandedAttributes(members, bindings, seen));
    }
  }
  return result;
}

function checkElement(node, imports, bindings, violations) {
  const name = node.name.name;
  const imported = imports.get(name);
  if (controls.has(name))
    issue(violations, node, 'control', 'Use a Core control; interactive DOM belongs in Core.');
  if (['style', 'link'].includes(name))
    issue(
      violations,
      node,
      'override',
      'Stylesheets are owned by the Core theme; consumers cannot inject them through JSX.',
    );
  for (const attribute of expandedAttributes(node.attributes, bindings)) {
    if (attribute.type !== 'JSXAttribute') continue;
    checkAppearance(attribute, name, imported, violations);
    // Placement belongs to a consumer-owned wrapper element, never to a Core className.
    if (attribute.name.name === 'className' && imported?.source.includes('/core/'))
      issue(
        violations,
        attribute,
        'override',
        'Core component appearance cannot be replaced with a caller className.',
      );
  }
}

function checkAppearance(attribute, name, imported, violations) {
  const prop = attribute.name.name;
  const allowedLayout =
    prop === 'style' &&
    /^[a-z]/u.test(name ?? '') &&
    !controls.has(name) &&
    inlineLayout(attribute);
  if (stylingProps.has(prop) && !allowedLayout) {
    issue(
      violations,
      attribute,
      'override',
      'Styling overrides belong in tokens / theme / the owning Core component.',
    );
  }
  if (
    /^(?:data-tone|data-size|data-variant|data-density|data-stretch)$/u.test(prop) &&
    imported?.source.includes('/core/')
  ) {
    issue(
      violations,
      attribute,
      'override',
      'Select the Core semantic prop instead of overriding its internal state attributes.',
    );
  }
  if (
    prop === 'contentEditable' ||
    (prop === 'role' && controls.has(attribute.value?.value ?? attribute.value?.expression?.value))
  ) {
    issue(violations, attribute, 'control', 'Editable and interactive roles belong in Core.');
  }
}

export function inspectScript(filename, source) {
  const core = filename.startsWith('core/');
  const test = /\.test\.tsx?$/u.test(filename);
  const violations = [];
  const ast = parse(source, { sourceType: 'module', plugins: ['typescript', 'jsx'] });
  const imports = new Map();
  const bindings = new Map();
  visit(ast, (node) => {
    if (node.type === 'VariableDeclarator' && node.id.type === 'Identifier')
      bindings.set(node.id.name, node.init);
    if (node.type === 'ImportDeclaration') {
      for (const specifier of node.specifiers)
        imports.set(specifier.local.name, {
          source: node.source.value,
          name: specifier.imported?.name ?? specifier.local.name,
        });
    }
  });
  if (filename.startsWith('core/ui/') && !test && !filename.includes('.stories.')) {
    checkCoreApi(ast, imports, violations);
  }
  visit(ast, (node) => {
    if (
      [
        'ImportDeclaration',
        'ExportNamedDeclaration',
        'ExportAllDeclaration',
        'CallExpression',
        'ImportExpression',
      ].includes(node.type)
    )
      checkLibrary(node, core, violations);
    if (
      !test &&
      filename !== 'core/theme/tokens.ts' &&
      ['StringLiteral', 'TemplateElement'].includes(node.type) &&
      colorPattern.test(node.value?.raw ?? node.value)
    ) {
      issue(violations, node, 'color', 'Define color literals in core/theme/tokens.ts.');
    }
    if (!core && !test && node.type === 'JSXOpeningElement')
      checkElement(node, imports, bindings, violations);
  });
  checkLayers(filename, ast, violations);
  checkAmbientTime(filename, ast, violations);
  return { violations };
}

// Inner Core layers, innermost first. Each may import only itself and the layers before it;
// none may import a package, so domain rules and ports stay free of React, React Flow,
// Tauri, theme tokens and components.
const innerLayers = ['domain', 'ports', 'application'];
// Adapters implement ports against the platform, never against the view.
const viewPackages =
  /^(?:react(?:-dom)?(?:$|\/)|react-aria-components|@xyflow\/react|react-i18next)/u;
// Only composition roots choose port implementations.
const compositionRoots = /^(?:app\/|screens\/|main\.tsx$)/u;
// Platform APIs are reached through the adapters that implement ports with them.
const platformPackages = /^@tauri-apps\//u;
// Browser timers have one implementation and no configuration. Inner layers receive a
// Scheduler or Clock; Core view hooks, features and adapters may default to these.
const ambientAdapters = new Set(['core/adapters/browser/timers']);

function coreLayer(filename) {
  const match = /^core\/(domain|ports|application|adapters)\//u.exec(filename);
  if (match) return match[1];
  return filename.startsWith('core/') ? 'ui' : undefined;
}

const declarations = new Set([
  'ImportDeclaration',
  'ExportNamedDeclaration',
  'ExportAllDeclaration',
  'ImportExpression',
]);

/** A string literal or a template literal without substitutions. */
function staticString(node) {
  if (node?.type === 'StringLiteral') return node.value;
  if (node?.type === 'TemplateLiteral' && node.expressions.length === 0)
    return node.quasis[0]?.value.cooked;
  return undefined;
}

/** `import.meta.glob(…)`, whose pattern names the modules it loads. */
function isImportMetaGlob(callee) {
  return (
    callee.type === 'MemberExpression' &&
    callee.object.type === 'MetaProperty' &&
    callee.property.name === 'glob'
  );
}

/** The module named by an import or export, `import()`, `require()`, `import x = require()`,
 * `import('…').T` or the pattern of `import.meta.glob()`. */
function moduleSource(node) {
  if (declarations.has(node.type)) return staticString(node.source);
  if (node.type === 'TSImportType') return staticString(node.source);
  if (node.type === 'TSExternalModuleReference') return staticString(node.expression);
  const call =
    node.type === 'CallExpression' &&
    (node.callee.type === 'Import' ||
      node.callee.name === 'require' ||
      isImportMetaGlob(node.callee));
  return call ? staticString(node.arguments[0]) : undefined;
}

function moduleSources(ast) {
  const sources = [];
  visit(ast, (node) => {
    const source = moduleSource(node);
    if (typeof source === 'string') sources.push({ node, source });
  });
  return sources;
}

/** `filename` and the result are relative to src; packages keep their bare name. */
function resolveSource(filename, source) {
  if (!source.startsWith('.')) return { package: source };
  const parts = filename.split('/').slice(0, -1);
  for (const part of source.split('/')) {
    if (part === '..') parts.pop();
    else if (part !== '.') parts.push(part);
  }
  return { module: parts.join('/') };
}

function innerLayerViolation(layer, target) {
  if (target.package) return `${layer} modules cannot import packages (${target.package}).`;
  const allowed = innerLayers.slice(0, innerLayers.indexOf(layer) + 1);
  return allowed.includes(coreLayer(target.module))
    ? undefined
    : `${layer} modules import only ${allowed.join(', ')} (not ${target.module}).`;
}

function adapterViolation(target) {
  if (target.package)
    return viewPackages.test(target.package)
      ? `adapters cannot import view packages (${target.package}).`
      : undefined;
  return coreLayer(target.module) === 'ui' || !target.module.startsWith('core/')
    ? `adapters import only domain, ports, application and adapters (not ${target.module}).`
    : undefined;
}

/** Core view code and everything outside Core reach adapters only through a composition
 * root, apart from the ambient allowlist, and never import platform APIs themselves. */
function compositionViolation(filename, target) {
  if (target.package)
    return platformPackages.test(target.package)
      ? `Only adapters import platform APIs (${target.package}).`
      : undefined;
  return target.module?.startsWith('core/adapters/') &&
    !ambientAdapters.has(target.module) &&
    !compositionRoots.test(filename)
    ? 'Port implementations are chosen by the app and screen composition roots.'
    : undefined;
}

function featureLocation(filename) {
  const match = /^features\/([^/]+)\/(?:(domain|ports|application|adapters)\/)?/u.exec(filename);
  return match ? { owner: match[1], layer: match[2] } : undefined;
}

function featureViolation(filename, target, strict = true) {
  const feature = featureLocation(filename);
  if (!feature) return undefined;
  const dependency = target.module && featureLocation(target.module);
  if (dependency && dependency.owner !== feature.owner)
    return 'Features are composed by screens and do not import another feature.';
  if (target.module && /^(?:app|screens)\//u.test(target.module))
    return 'Features cannot import app or screen composition roots.';
  if (!strict || !innerLayers.includes(feature.layer)) return undefined;
  if (target.package) return `${feature.layer} modules cannot import packages (${target.package}).`;
  const allowed = innerLayers.slice(0, innerLayers.indexOf(feature.layer) + 1);
  const targetLayer = dependency?.layer ?? coreLayer(target.module);
  return allowed.includes(targetLayer)
    ? undefined
    : `${feature.layer} modules import only ${allowed.join(', ')} (not ${target.module}).`;
}

function layerViolation(filename, target) {
  if (target.module?.startsWith('test-support/') && !filename.startsWith('test-support/'))
    return 'Test support is for tests and stories only.';
  const featureIssue = featureViolation(filename, target);
  if (featureIssue) return featureIssue;
  if (filename.startsWith('core/') && /^(?:app|screens|features)\//u.test(target.module ?? ''))
    return 'Core cannot import features or composition roots.';
  const layer = coreLayer(filename);
  if (innerLayers.includes(layer)) return innerLayerViolation(layer, target);
  if (layer === 'adapters') return adapterViolation(target);
  return compositionViolation(filename, target);
}

function checkLayers(filename, ast, violations) {
  const story = /\.stories\.tsx?$/u.test(filename);
  const test = /\.test\.tsx?$/u.test(filename);
  for (const { node, source } of moduleSources(ast)) {
    const target = resolveSource(filename, source);
    // Stories assemble product examples; tests retain the direction of their owning layer.
    const message = story
      ? undefined
      : test
        ? featureViolation(filename, target, false)
        : layerViolation(filename, target);
    if (message) issue(violations, node, 'layer', message);
  }
}

function checkAmbientTime(filename, ast, violations) {
  if (/\.(?:test|stories)\.tsx?$/u.test(filename)) return;
  if (!filename.startsWith('features/') && !innerLayers.includes(coreLayer(filename))) return;
  const timers = new Set(['setTimeout', 'clearTimeout', 'setInterval', 'clearInterval']);
  visit(ast, (node) => {
    const timer = node.type === 'Identifier' && timers.has(node.name);
    const member = node.type === 'MemberExpression' || node.type === 'OptionalMemberExpression';
    const property = member && (node.computed ? staticString(node.property) : node.property.name);
    const object = member && node.object.type === 'Identifier' && node.object.name;
    const clock = property === 'now' && ['Date', 'performance'].includes(object);
    if (timer || clock)
      issue(violations, node, 'time', 'Inject Scheduler or Clock instead of reading global time.');
  });
}

function checkCoreApi(ast, imports, violations) {
  visit(ast, (node) => {
    if (node.type !== 'TSTypeReference') return;
    const imported = imports.get(node.typeName.name);
    if (imported?.source.startsWith('@mantine/') && imported.name.endsWith('Props')) {
      issue(
        violations,
        node,
        'api',
        'Core must expose an explicit product contract instead of all library props.',
      );
    }
    if (node.typeName.name === 'Pick') {
      const keys = node.typeArguments?.params[1];
      visit(keys, (key) => {
        if (
          key.type === 'StringLiteral' &&
          [...stylingProps, 'className', 'color', 'c', 'size', 'radius', 'variant'].includes(
            key.value,
          )
        ) {
          issue(
            violations,
            key,
            'api',
            'Do not expose library appearance props through the Core contract.',
          );
        }
      });
      return false;
    }
  });
}

function checkSelector(node, violations) {
  css.walk(node.prelude, (part) => {
    if (
      part.type === 'TypeSelector' &&
      (controls.has(part.name.toLowerCase()) ||
        ['html', 'body', '*'].includes(part.name.toLowerCase()))
    ) {
      issue(
        violations,
        part,
        'selector',
        'Consumer CSS cannot target controls or global elements.',
      );
    }
    if (
      part.type === 'PseudoClassSelector' &&
      ['root', 'global'].includes(part.name.toLowerCase())
    ) {
      issue(violations, part, 'selector', 'Global / library / Core selectors belong in Core.');
    }
    if (
      part.type === 'ClassSelector' &&
      /^(?:mantine-|react-flow|action-button)/u.test(part.name)
    ) {
      issue(
        violations,
        part,
        'selector',
        'Library / Core internals cannot be styled by consumers.',
      );
    }
    if (part.type === 'AttributeSelector') checkAttributeSelector(part, violations);
  });
}

function checkAttributeSelector(part, violations) {
  if (
    /^(?:class|style|data-tone|data-size|data-variant|data-density|data-stretch)$/u.test(
      part.name.name,
    )
  ) {
    issue(
      violations,
      part,
      'selector',
      'Core implementation attributes cannot be styled by consumers.',
    );
  }
  if (
    part.name.name === 'role' &&
    /^(?:button|checkbox|switch|slider|option|menuitem|textbox|combobox)$/u.test(
      part.value?.value ?? part.value?.name,
    )
  ) {
    issue(
      violations,
      part,
      'selector',
      'Interactive roles are styled by their owning Core component.',
    );
  }
}

const namedColorExceptions = new Set(['transparent', 'currentcolor', 'inherit']);

function hasNamedColor(value) {
  let named = false;
  css.walk(value, (part) => {
    if (
      part.type === 'Identifier' &&
      !namedColorExceptions.has(part.name.toLowerCase()) &&
      css.lexer.matchType('color', part.name).matched
    )
      named = true;
  });
  return named;
}

/** One entry per declaration rule; `consumer` rules apply outside Core only. */
const declarationRules = [
  {
    consumer: true,
    code: 'important',
    message: 'Change the owning contract instead of adding !important.',
    test: (node) => node.important,
  },
  {
    consumer: true,
    code: 'token',
    message: 'Visual tokens are defined in Core; only --layout-* variables may be local.',
    test: (node) => node.property.startsWith('--') && !node.property.startsWith('--layout-'),
  },
  {
    code: 'token',
    message: 'Layout variables cannot replace visual role tokens.',
    test: (node, value) => !layoutProperties.has(node.property) && /var\(--layout-/u.test(value),
  },
  {
    code: 'typography',
    message: 'Use a typography role from core/theme/tokens.ts.',
    test: (node, value) =>
      ['font-size', 'font-family'].includes(node.property) &&
      !/^(?:var\(--[\w-]+\)|inherit)$/u.test(value),
  },
  {
    code: 'spacing',
    message: 'Spacing uses a --space-* step or a layout role from core/theme/tokens.ts.',
    test: (node) =>
      spacingProperty.test(node.property.toLowerCase()) && hasLiteralLength(node.value),
  },
  {
    code: 'layer',
    message: 'Stacking order uses a --layer-* token from tokens.ts.',
    test: (node, value) =>
      node.property === 'z-index' && !/^(?:auto|var\(--layer-[\w-]+\))$/u.test(value),
  },
  {
    code: 'motion',
    message: 'Durations use a --motion-* token so reduced motion applies.',
    test: (node) => motionProperties.has(node.property) && hasLiteralDuration(node.value),
  },
  {
    code: 'typography',
    message: 'The font shorthand also takes size and family roles.',
    test: (node) => node.property === 'font' && hasLiteralFontPart(node.value),
  },
  {
    code: 'color',
    message: 'Define color literals in core/theme/tokens.ts.',
    test: (_node, value) => colorPattern.test(value),
  },
  {
    code: 'color',
    message: 'Named colors also belong in core/theme/tokens.ts.',
    test: (node) => hasNamedColor(node.value),
  },
];

function checkDeclaration(node, core, violations, atrule) {
  if (isCoreFontDescriptor(core, atrule)) return;
  const value = css.generate(node.value);
  for (const rule of declarationRules)
    if (!(rule.consumer && core) && rule.test(node, value))
      issue(violations, node, rule.code, rule.message);
}

const fontKeywords = new Set([
  'inherit',
  'initial',
  'unset',
  'revert',
  'revert-layer',
  'normal',
  'italic',
  'oblique',
  'bold',
  'bolder',
  'lighter',
  'small-caps',
]);

const motionProperties = new Set([
  'transition',
  'transition-duration',
  'transition-delay',
  'animation',
  'animation-duration',
  'animation-delay',
]);

const spacingProperty =
  /^(?:(?:padding|margin|scroll-padding|scroll-margin)(?:-[a-z-]+)?|(?:grid-)?(?:row-|column-)?gap)$/u;

/** Any non-zero length, including one hidden in a var() fallback. Ratios inside calc()
 * (e.g. `/ 2`, `-1 *`) are numbers, not lengths; `1lh` measures the text line when
 * centring it in a size step. */
function hasLiteralLength(value) {
  let literal = false;
  css.walk(value, (part) => {
    if (
      part.type === 'Dimension' &&
      Number(part.value) !== 0 &&
      !(part.unit === 'lh' && Number(part.value) === 1)
    )
      literal = true;
    if (part.type === 'Raw' && part.value.trim()) {
      try {
        literal ||= hasLiteralLength(
          css.parse(part.value, { context: 'value', onParseError: fail }),
        );
      } catch {
        literal = true;
      }
    }
  });
  return literal;
}

function hasLiteralDuration(value) {
  let literal = false;
  css.walk(value, (part) => {
    if (part.type === 'Dimension' && /^m?s$/iu.test(part.unit)) literal = true;
    // var() fallbacks arrive as Raw text; unparseable text is not a motion token either.
    if (part.type === 'Raw' && part.value.trim()) {
      try {
        literal ||= hasLiteralDuration(
          css.parse(part.value, { context: 'value', onParseError: fail }),
        );
      } catch {
        literal = true;
      }
    }
  });
  return literal;
}

/** Fallback grammar is wider than a value; text the parser rejects is not a role either. */
function hasLiteralFallback(raw) {
  try {
    return hasLiteralFontPart(css.parse(raw, { context: 'value', onParseError: fail }));
  } catch {
    return true;
  }
}

function fail(error) {
  throw error;
}

/** Weights and unitless line heights may be literal; sizes and families are roles.
 * var() fallbacks arrive as Raw text and are parsed so they cannot hide literals. */
function hasLiteralFontPart(value) {
  let literal = false;
  css.walk(value, (part) => {
    if (part.type === 'Raw' && part.value.trim()) literal ||= hasLiteralFallback(part.value);
    if (['Dimension', 'Percentage', 'String'].includes(part.type)) literal = true;
    if (
      part.type === 'Identifier' &&
      !part.name.startsWith('--') &&
      !fontKeywords.has(part.name.toLowerCase())
    )
      literal = true;
  });
  return literal;
}

function isCoreFontDescriptor(core, atrule) {
  return core && atrule?.name === 'font-face';
}

export function inspectCss(filename, source) {
  const core = filename.startsWith('core/');
  const violations = [];
  const ast = css.parse(source, { positions: true });
  css.walk(ast, function inspect(node) {
    if (!core && node.type === 'Atrule' && node.name === 'import')
      issue(violations, node, 'selector', 'Consumer CSS cannot import global styles.');
    if (node.type === 'Rule' && !core) checkSelector(node, violations);
    if (node.type !== 'Declaration') return;
    // Font-face descriptors declare the bundled family; they are not consumer typography.
    checkDeclaration(node, core, violations, this.atrule);
  });
  return violations;
}

// Variables that React Aria writes onto its overlay elements at runtime.
const libraryVariables = new Set(['--trigger-width', '--available-height']);
const variableReference = /var\(\s*(--[\w-]+)/gu;

/** Raw CSS with comments and quoted strings blanked, scanned left to right so a quote
 * inside a comment or a comment marker inside a string is read in its own context. */
function strippedRaw(text) {
  let output = '';
  let index = 0;
  while (index < text.length) {
    if (text.startsWith('/*', index)) {
      const end = text.indexOf('*/', index + 2);
      index = end < 0 ? text.length : end + 2;
      output += ' ';
    } else if (text[index] === '"' || text[index] === "'") {
      index = stringEnd(text, index);
      output += ' ';
    } else {
      output += text[index];
      index += 1;
    }
  }
  return output;
}

function stringEnd(text, start) {
  let index = start + 1;
  while (index < text.length && text[index] !== text[start]) index += text[index] === '\\' ? 2 : 1;
  return index + 1;
}

function varName(part) {
  if (part.type !== 'Function' || part.name.toLowerCase() !== 'var') return undefined;
  const name = part.children.first;
  return name?.type === 'Identifier' && name.name.startsWith('--') ? name.name : undefined;
}

/** Custom-property values and var() fallbacks arrive as Raw text. Parsing drops comments
 * and keeps strings as strings; token-stream blocks that are not a value
 * (`--data: { color: var(--x) }`) are scanned without comments or strings. */
function rawReferences(raw, line, referenced) {
  try {
    cssValueReferences(css.parse(raw, { context: 'value' }), line, referenced);
  } catch {
    for (const match of strippedRaw(raw).matchAll(variableReference))
      referenced.push({ name: match[1], line });
  }
}

function cssValueReferences(value, line, referenced) {
  css.walk(value, (part) => {
    const name = varName(part);
    if (name) referenced.push({ name, line });
    if (part.type === 'Raw' && part.value.trim()) rawReferences(part.value, line, referenced);
  });
}

/** Custom properties a stylesheet defines (declarations and @property) and the
 * var() names it reads, including names inside var() fallbacks. */
function cssVariables(source) {
  const defined = [];
  const referenced = [];
  const ast = css.parse(source, { positions: true, parseCustomProperty: false });
  css.walk(ast, (node) => {
    if (node.type === 'Declaration' && node.property.startsWith('--')) defined.push(node.property);
    if (node.type === 'Atrule' && node.name === 'property' && node.prelude)
      defined.push(css.generate(node.prelude).trim());
    if (node.type === 'Declaration')
      cssValueReferences(node.value, node.loc?.start.line ?? 1, referenced);
  });
  return { defined, referenced };
}

const members = new Set(['MemberExpression', 'OptionalMemberExpression']);

function memberName(node) {
  return node.computed ? staticKey(node.property) : node.property?.name;
}

/** The custom property named by `<…>.style.setProperty('--x', …)`, including computed and
 * optional forms. setProperty on anything other than a style declaration defines nothing. */
function styleSetProperty(node) {
  if (node.type !== 'CallExpression' && node.type !== 'OptionalCallExpression') return undefined;
  const callee = node.callee;
  if (!members.has(callee.type) || memberName(callee) !== 'setProperty') return undefined;
  const target = callee.object;
  const style = members.has(target.type)
    ? memberName(target) === 'style'
    : target.type === 'Identifier' && target.name === 'style';
  return style ? staticKey(node.arguments[0]) : undefined;
}

function staticKey(node) {
  if (node?.type === 'StringLiteral') return node.value;
  if (node?.type === 'TemplateLiteral' && node.expressions.length === 0)
    return node.quasis[0]?.value.cooked;
  return undefined;
}

function literalTexts(node) {
  if (node.type === 'StringLiteral') return [node.value];
  if (node.type === 'TemplateLiteral') return node.quasis.map((quasi) => quasi.value.cooked ?? '');
  return [];
}

/** Inline-style keys and style.setProperty() names define variables; var() inside string
 * or template text references them. getPropertyValue() reads and defines nothing. */
function scriptVariables(source) {
  const defined = [];
  const referenced = [];
  const ast = parse(source, { sourceType: 'module', plugins: ['typescript', 'jsx'] });
  visit(ast, (node) => {
    if (node.type === 'ObjectProperty') {
      const key = staticKey(node.key);
      if (key?.startsWith('--')) defined.push(key);
    }
    const property = styleSetProperty(node);
    if (property?.startsWith('--')) defined.push(property);
    for (const text of literalTexts(node))
      for (const match of text.matchAll(variableReference))
        referenced.push({ name: match[1], line: node.loc?.start.line ?? 1 });
  });
  return { defined, referenced };
}

/** Every var(--name) used by product code must be defined by tokens, a stylesheet,
 * @property or an inline style. Removed aliases and misspelled roles therefore fail
 * instead of falling back silently. Tests are excluded as both sources and definitions. */
export function inspectVariableReferences(files) {
  const product = files.filter(({ filename }) => !/\.(test|spec)\.[cm]?[jt]sx?$/u.test(filename));
  const scanned = product.map(({ filename, source }) => ({
    filename,
    ...(filename.endsWith('.css') ? cssVariables(source) : scriptVariables(source)),
  }));
  const defined = new Set(libraryVariables);
  for (const file of scanned) for (const name of file.defined) defined.add(name);
  const violations = [];
  for (const { filename, referenced } of scanned)
    for (const { name, line } of referenced)
      if (!defined.has(name))
        violations.push({
          filename,
          line,
          code: 'token',
          message: `${name} is not defined by tokens, a stylesheet or an inline style.`,
        });
  return violations;
}

// Screen owners and their module basenames; a path matches only that exact module.
const screenModules = ['App', 'StudioScreen', 'RoutingWorkspace'];

function screenModule(source) {
  return screenModules.some((name) => new RegExp(`(?:^|/)${name}$`, 'u').test(source));
}

/** Only a bare side-effect import is guaranteed to execute the module: unused value
 * imports may be dropped by the transform, and type imports are always erased. */
function sideEffectImport(statement) {
  return statement.importKind !== 'type' && statement.specifiers.length === 0;
}

function rendersScreen(statement) {
  if (!screenModule(statement.source.value) || statement.importKind === 'type') return false;
  return statement.specifiers.some(
    (specifier) =>
      specifier.importKind !== 'type' &&
      (specifier.type === 'ImportNamespaceSpecifier' ||
        screenModules.includes(specifier.imported?.name)),
  );
}

function testAst(filename, source) {
  if (!/\.test\.tsx?$/u.test(filename)) return undefined;
  return parse(source, { sourceType: 'module', plugins: ['typescript', 'jsx'] });
}

/** `import()` of a screen loads it as surely as a static value import. */
function importsScreenDynamically(node) {
  const dynamic =
    node.type === 'ImportExpression' ||
    (node.type === 'CallExpression' && node.callee.type === 'Import');
  return dynamic && screenModule(moduleSource(node) ?? '');
}

/** The first static or dynamic import through which a test can render a whole screen. */
function screenImport(ast) {
  let found;
  visit(ast, (node) => {
    if (found) return false;
    if (node.type === 'ImportDeclaration' ? rendersScreen(node) : importsScreenDynamically(node))
      found = node;
  });
  return found;
}

/** A test that renders a whole screen (App, StudioScreen, RoutingWorkspace) imports
 * test-support/integration, which owns the integration-test timeout. Unit tests keep
 * the default, so a screen-level test cannot silently run under the unit budget. */
export function inspectIntegrationTests(files) {
  const violations = [];
  for (const { filename, source } of files) {
    const ast = testAst(filename, source);
    const screen = ast && screenImport(ast);
    if (!screen) continue;
    const integration = ast.program.body.some(
      (statement) =>
        statement.type === 'ImportDeclaration' &&
        statement.source.value.endsWith('test-support/integration') &&
        sideEffectImport(statement),
    );
    if (integration) continue;
    violations.push({
      filename,
      line: screen.loc?.start?.line ?? 1,
      code: 'test',
      message:
        "Screen-level tests add `import '…/test-support/integration'` for the integration timeout.",
    });
  }
  return violations;
}

/** The only test files (relative to src) that may render a whole screen. They cover
 * composition, wiring, focus and keyboard; logic is extracted into a pure module with fast
 * unit tests instead of joining this list. */
export const screenRenderTests = new Set([
  'app/App.midi.a11y.test.tsx',
  'app/App.midi.test.tsx',
  'app/App.test.tsx',
  'app/AppAbout.test.tsx',
  'app/DriverNodeSettings.test.tsx',
  'features/routing-workspace/DeviceAppActivity.integration.test.tsx',
  'features/routing-workspace/ProcessorReport.integration.test.tsx',
  'features/routing-workspace/RoutingWorkspace.focus.test.tsx',
  'features/routing-workspace/RoutingWorkspace.contract.test.tsx',
  'features/routing-workspace/applicationReselection.test.tsx',
  'features/routing-workspace/virtualNodeStatus.test.tsx',
  'screens/RoutingWorkspace.integration.test.tsx',
  'screens/RoutingWorkspaceProcessorIntent.test.tsx',
  'screens/StudioDiagnostics.integration.test.tsx',
  'screens/StudioEngineStatus.test.tsx',
  'screens/StudioScreen.contract.test.tsx',
  'screens/StudioScreen.creationBoundary.test.tsx',
  'screens/StudioScreen.test.tsx',
]);

/** Whole-screen renders only in allowlisted tests, and no allowlist entry outlives its
 * screen render. `files` carry src-relative filenames. */
export function inspectScreenRenderTests(files, allowed = screenRenderTests) {
  const violations = [];
  const renderers = new Set();
  for (const { filename, source } of files) {
    const ast = testAst(filename, source);
    const screen = ast && screenImport(ast);
    if (!screen) continue;
    renderers.add(filename);
    if (allowed.has(filename)) continue;
    violations.push({
      filename,
      line: screen.loc?.start?.line ?? 1,
      code: 'test',
      message:
        'Only allowlisted composition tests render a whole screen; test logic through a pure module.',
    });
  }
  for (const filename of allowed)
    if (!renderers.has(filename))
      violations.push({
        filename,
        line: 1,
        code: 'test',
        message: 'This test no longer renders a whole screen; remove it from screenRenderTests.',
      });
  return violations;
}
