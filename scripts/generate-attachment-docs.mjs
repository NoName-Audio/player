import ts from 'typescript';
import { createRequire } from 'node:module';
import { execFileSync } from 'node:child_process';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

// Use the same installed Vue/Volar compiler stack as vue-tsc, not an SFC shim.
const vueRequire = createRequire(import.meta.resolve('vue-tsc'));
const vue = vueRequire('@vue/language-core');
const { proxyCreateProgram } = vueRequire('@volar/typescript/lib/node/proxyCreateProgram');

export function extractAttachments(root = process.cwd()) {
  const config = path.join(root, 'tsconfig.json');
  const parsed = vue.createParsedCommandLine(ts, ts.sys, config);
  if (parsed.errors.length) throw new Error(ts.formatDiagnosticsWithColorAndContext(parsed.errors, {
    getCanonicalFileName: f => f, getCurrentDirectory: () => root, getNewLine: () => '\n',
  }));
  const create = proxyCreateProgram(ts, ts.createProgram, () => ({ languagePlugins: [
    vue.createVueLanguagePlugin(ts, parsed.options, parsed.vueOptions, id => id),
  ] }));
  const program = create({ rootNames: parsed.fileNames, options: parsed.options, host: ts.createCompilerHost(parsed.options) });
  const checker = program.getTypeChecker();
  const canonical = path.resolve(root, 'src/plugins/withPluginAttachments.ts');
  const implementation = program.getSourceFile(canonical);
  if (!implementation) throw new Error(`Missing attachment API: ${canonical}`);
  const api = checker.getExportsOfModule(checker.getSymbolAtLocation(implementation)).find(s => s.name === 'withPluginAttachments');
  const resolve = symbol => {
    const seen = new Set();
    while (symbol && !seen.has(symbol)) {
      seen.add(symbol);
      if (symbol.flags & ts.SymbolFlags.Alias) symbol = checker.getAliasedSymbol(symbol);
      else {
        const declaration = symbol.valueDeclaration;
        if (declaration && ts.isVariableDeclaration(declaration) && declaration.initializer) {
          const next = checker.getSymbolAtLocation(declaration.initializer);
          if (next && next !== symbol) { symbol = next; continue; }
        }
        break;
      }
    }
    return symbol;
  };
  const targets = [];
  const fail = (node, message) => {
    const source = node.getSourceFile();
    const point = source.getLineAndCharacterOfPosition(node.getStart());
    throw new Error(`${path.relative(root, source.fileName)}:${point.line + 1}:${point.character + 1}: ${message}`);
  };
  const describe = symbol => ts.displayPartsToString(symbol?.getDocumentationComment(checker) ?? []);
  const fields = (type, location, prefix = '', ancestors = new Set()) => {
    if (type.flags & (ts.TypeFlags.Any | ts.TypeFlags.Unknown)) fail(location, `Context ${prefix || 'return value'} has unresolved any/unknown type; provide explicit serializable prop/context types.`);
    if (ancestors.has(type)) return [];
    const next = new Set(ancestors).add(type);
    const result = [];
    const members = checker.getPropertiesOfType(type).sort((a, b) => a.name.localeCompare(b.name, 'en'));
    for (const member of members) {
      const memberType = checker.getTypeOfSymbolAtLocation(member, location);
      const name = prefix ? `${prefix}.${member.name}` : member.name;
      const text = checker.typeToString(memberType, location, ts.TypeFormatFlags.NoTruncation | ts.TypeFormatFlags.UseAliasDefinedOutsideCurrentScope);
      result.push({ name, type: text, optional: !!(member.flags & ts.SymbolFlags.Optional), description: describe(member) });
      const nonNull = checker.getNonNullableType(memberType);
      if (memberType.flags & (ts.TypeFlags.Any | ts.TypeFlags.Unknown)) fail(location, `Context field ${name} has unresolved ${text}; provide an explicit type.`);
      const element = checker.isArrayType(nonNull) ? checker.getTypeArguments(nonNull)[0] : undefined;
      const nested = element ?? nonNull;
      if ((nested.flags & ts.TypeFlags.Object) && !checker.getSignaturesOfType(nested, ts.SignatureKind.Call).length) {
        result.push(...fields(nested, location, element ? `${name}[]` : name, next));
      }
    }
    return result;
  };
  for (const source of program.getSourceFiles()) {
    if (source.isDeclarationFile || source.fileName.includes('/node_modules/')) continue;
    const visit = node => {
      if (ts.isCallExpression(node) && resolve(checker.getSymbolAtLocation(node.expression)) === api) {
        if (node.arguments.length !== 2 || !ts.isObjectLiteralExpression(node.arguments[1])) fail(node, 'withPluginAttachments requires inline static options { target, context }; computed options cannot be documented.');
        const options = node.arguments[1];
        const properties = new Map();
        for (const property of options.properties) {
          if (!ts.isPropertyAssignment(property) || !property.name || ts.isComputedPropertyName(property.name)) fail(property, 'Attachment options must use explicit static property assignments, without spreads or computed names.');
          properties.set(property.name.text, property.initializer);
        }
        const target = properties.get('target');
        const context = properties.get('context');
        if (!target || !ts.isStringLiteralLike(target) || !/^[A-Za-z][A-Za-z0-9.-]{0,63}$/.test(target.text)) fail(target ?? node, 'Attachment target must be a literal ID matching [A-Za-z][A-Za-z0-9.-]{0,63}.');
        if (!context) fail(node, 'Attachment registration is missing context.');
        if (targets.some(item => item.id === target.text)) fail(target, `Duplicate attachment target ID ${target.text}. Give each registration a unique ID.`);
        const signatures = checker.getSignaturesOfType(checker.getTypeAtLocation(context), ts.SignatureKind.Call);
        if (signatures.length !== 1) fail(context, 'Context must have one statically resolvable call signature.');
        const diagnostics = program.getSemanticDiagnostics(source).filter(d => d.start >= node.pos && d.start < node.end);
        if (diagnostics.length) fail(node, ts.flattenDiagnosticMessageText(diagnostics[0].messageText, '\n'));
        let documented = node;
        while (documented.parent && !ts.isSourceFile(documented.parent) && !ts.isBlock(documented.parent)) documented = documented.parent;
        const description = (ts.getJSDocCommentsAndTags(documented).filter(ts.isJSDoc).map(doc => typeof doc.comment === 'string' ? doc.comment : ts.displayPartsToString(doc.comment ?? [])).join('\n'));
        const line = source.getLineAndCharacterOfPosition(node.getStart()).line + 1;
        targets.push({ id: target.text, source: `${path.relative(root, source.fileName).split(path.sep).join('/')}:${line}`, description, fields: fields(checker.getReturnTypeOfSignature(signatures[0]), context) });
      }
      ts.forEachChild(node, visit);
    };
    visit(source);
  }
  return targets.sort((a, b) => a.id.localeCompare(b.id, 'en'));
}

export function renderAttachments(targets, provenance) {
  const escape = value => value.replaceAll('|', '\\|').replaceAll('\n', ' ');
  return ['# Attachment targets', '', '<!-- Generated by Player scripts/generate-attachment-docs.mjs. Do not edit by hand. -->', '',
    `Player version: **${provenance.version}**`, `Player commit: \`${provenance.commit}\``, `Source status: **${provenance.status}**`, '',
    'Targets are discovered from Player registrations using the TypeScript checker and Vue language tooling. All targets support `before` and `after`; context is a JSON snapshot, not a live Vue object.', '',
    ...targets.flatMap(target => [`## ${target.id}`, '', target.description || 'No registration description provided.', '', `Source: \`${target.source}\``, '', 'Positions: `before`, `after`.', '', '| Context field | Type | Description |', '| --- | --- | --- |', ...target.fields.map(field => `| \`${field.name}${field.optional ? '?' : ''}\` | \`${escape(field.type)}\` | ${escape(field.description)} |`), '']),
  ].join('\n');
}

export function generateAttachments({ root = process.cwd(), output = 'plugins/docs/attachment-targets.md' } = {}) {
  const git = (...args) => execFileSync('git', ['-C', root, ...args], { encoding: 'utf8' }).trim();
  const version = JSON.parse(readFileSync(path.join(root, 'package.json'), 'utf8')).version;
  const commit = git('rev-parse', 'HEAD');
  // Ignore only this generated destination; existing user/submodule changes remain visible.
  const relativeOutput = path.relative(root, path.resolve(root, output)).split(path.sep).join('/');
  const dirty = git('status', '--porcelain', '--untracked-files=normal', '--', '.', `:(exclude)${relativeOutput}`);
  const status = dirty ? 'dirty/local checkout (not a release snapshot)' : 'clean checkout';
  const targets = extractAttachments(root);
  const destination = path.resolve(root, output);
  mkdirSync(path.dirname(destination), { recursive: true });
  writeFileSync(destination, renderAttachments(targets, { version, commit, status }));
  return { destination, targets, version, commit, status };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const args = process.argv.slice(2).filter(arg => arg !== '--');
    if (args.length && (args.length !== 2 || args[0] !== '--output')) throw new Error('Usage: node scripts/generate-attachment-docs.mjs [--output PATH]');
    const result = generateAttachments({ output: args[1] });
    console.log(`Generated ${result.targets.length} attachment target(s): ${result.destination} (${result.status})`);
  } catch (error) { console.error(error.message); process.exitCode = 1; }
}
