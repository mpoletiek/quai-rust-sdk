import ts from 'typescript';
import { portableInventory } from './inventory-paths.mjs';
import { readFileSync, writeFileSync, existsSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';

const base = fileURLToPath(new URL('../node_modules/quais/', import.meta.url));
const metadata = JSON.parse(readFileSync(path.join(base, 'package.json'), 'utf8'));
const roots = Object.entries(metadata.exports).map(([subpath, conditions]) => ({
  subpath,
  conditions,
  declaration: conditions.import.replace(/\.js$/, '.d.ts'),
}));
for (const root of roots) {
  if (!existsSync(path.join(base, root.declaration))) throw new Error(`Missing export declaration: ${root.subpath}`);
}
const program = ts.createProgram(roots.map(root => path.join(base, root.declaration)), {
  target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.NodeNext,
  moduleResolution: ts.ModuleResolutionKind.NodeNext, skipLibCheck: true, noEmit: true,
});
const checker = program.getTypeChecker();
const format = ts.TypeFormatFlags.NoTruncation | ts.TypeFormatFlags.UseAliasDefinedOutsideCurrentScope;
const location = declaration => {
  const source = declaration.getSourceFile();
  return { file: path.relative(base, source.fileName).replaceAll(path.sep, '/'), line: source.getLineAndCharacterOfPosition(declaration.getStart()).line + 1 };
};
const visible = symbol => !symbol.getName().startsWith('#') && !(symbol.declarations ?? []).some(decl => {
  const flags = ts.getCombinedModifierFlags(decl);
  return flags & (ts.ModifierFlags.Private | ts.ModifierFlags.Protected);
});
const describe = symbol => {
  const declaration = symbol.valueDeclaration ?? symbol.declarations?.[0];
  if (!declaration) return { name: symbol.getName(), kind: 'synthetic' };
  const type = checker.getTypeOfSymbolAtLocation(symbol, declaration);
  const signatures = [
    ...type.getCallSignatures().map(sig => checker.signatureToString(sig, declaration, format)),
    ...type.getConstructSignatures().map(sig => `new ${checker.signatureToString(sig, declaration, format)}`),
  ];
  return {
    name: symbol.getName(), kind: ts.SyntaxKind[declaration.kind],
    declarations: (symbol.declarations ?? []).map(location),
    ...(signatures.length ? { signatures } : { type: checker.typeToString(type, declaration, format) }),
  };
};
const exports = [];
for (const root of roots) {
  const source = program.getSourceFile(path.join(base, root.declaration));
  const module = checker.getSymbolAtLocation(source);
  if (!module) throw new Error(`No module symbol: ${root.subpath}`);
  for (const symbol of checker.getExportsOfModule(module).sort((a, b) => a.name.localeCompare(b.name, 'en'))) {
    const target = symbol.flags & ts.SymbolFlags.Alias ? checker.getAliasedSymbol(symbol) : symbol;
    const record = { id: `${root.subpath}:${symbol.name}`, subpath: root.subpath, exportedName: symbol.name, target: describe(target) };
    if (target.flags & (ts.SymbolFlags.TypeAlias | ts.SymbolFlags.Interface | ts.SymbolFlags.Enum)) {
      record.target.definition = target.declarations.map(declaration => declaration.getText()).join('\n');
      delete record.target.type;
    }
    if (target.flags & ts.SymbolFlags.Class) {
      const declaration = target.valueDeclaration ?? target.declarations[0];
      const instance = checker.getDeclaredTypeOfSymbol(target);
      const statics = checker.getTypeOfSymbolAtLocation(target, declaration);
      record.instanceMembers = checker.getPropertiesOfType(instance).filter(visible).map(describe).sort((a, b) => a.name.localeCompare(b.name, 'en'));
      record.staticMembers = checker.getPropertiesOfType(statics).filter(symbol => symbol.name !== 'prototype' && visible(symbol)).map(describe).sort((a, b) => a.name.localeCompare(b.name, 'en'));
    }
    if (target.flags & ts.SymbolFlags.NamespaceModule) {
      record.namespaceExports = checker.getExportsOfModule(target).map(symbol => symbol.name).sort();
    }
    exports.push(record);
  }
}
const declarations = program.getSourceFiles().filter(source => source.fileName.startsWith(base)).map(source => ({
  file: path.relative(base, source.fileName).replaceAll(path.sep, '/'),
  sha256: createHash('sha256').update(readFileSync(source.fileName)).digest('hex'),
})).sort((a, b) => a.file.localeCompare(b.file, 'en'));
const result = {
  schemaVersion: 1, reference: `quais@${metadata.version}`, typescript: ts.version,
  scope: 'Shipped package export-map ESM declarations resolved with the TypeScript compiler checker. Aliases retain export-root identity. Public inherited class instance/static members and overload signatures are included. Root quais namespace lists its names; definitions are also represented as root exports.',
  limitations: [
    'Declaration surface inventory, not proof of behavioral parity or implementation coverage.',
    'CJS conditions are recorded; declaration surfaces are enumerated from ESM conditions only.',
    'No browser-condition runtime equivalence, undocumented deep imports, computed runtime members, interface-member expansion, or semantic typechecking of all upstream dependencies.',
    'A Rust mapping and per-feature conformance status must be maintained separately as implementation progresses.',
  ],
  roots, declarations,
  counts: { roots: roots.length, exports: exports.length, classes: exports.filter(entry => entry.instanceMembers).length, publicClassMembers: exports.reduce((count, entry) => count + (entry.instanceMembers?.length ?? 0) + (entry.staticMembers?.length ?? 0), 0) },
  exports,
};
const syntactic = program.getSyntacticDiagnostics();
if (syntactic.length) throw new Error(`Reference has ${syntactic.length} syntax diagnostics`);
writeFileSync(new URL('../api-inventory.json', import.meta.url), `${JSON.stringify(portableInventory(result, base), null, 2)}\n`);
console.log(JSON.stringify(result.counts));
