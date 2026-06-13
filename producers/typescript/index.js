#!/usr/bin/env node
"use strict";

const fs = require("node:fs");
const path = require("node:path");

const PRODUCER = "code-intelligence-external-typescript";
const EXTENSIONS = new Set([".ts", ".tsx", ".js", ".jsx", ".mts", ".cts", ".mjs", ".cjs"]);
const SKIP_DIRS = new Set([".git", "node_modules", "dist", "build", "target", "vendor", ".next", "coverage"]);
const CALL_KEYWORDS = new Set(["if", "for", "while", "switch", "function", "constructor"]);
const BINDING_WORDS = new Set(["const", "let", "var", "function", "class"]);

function loadProjectTypescript(root) {
	try {
		return require(require.resolve("typescript", { paths: [root] }));
	} catch {
		return null;
	}
}

function usage() {
	console.error(`usage: ${PRODUCER} index --output <normalized-json>`);
	return 64;
}

function stableId(language, filePath, kind, qualifiedName, startLine, startByte) {
	return `${language}:${filePath}:${kind}:${qualifiedName}:${startLine || 0}:${startByte || 0}`;
}

function discoverFiles(root) {
	const found = [];
	function walk(dir) {
		for (const name of fs.readdirSync(dir).sort()) {
			if (SKIP_DIRS.has(name)) continue;
			const full = path.join(dir, name);
			const stat = fs.statSync(full);
			if (stat.isDirectory()) walk(full);
			if (stat.isFile() && EXTENSIONS.has(path.extname(name))) found.push(path.relative(root, full).split(path.sep).join("/"));
		}
	}
	walk(root);
	return found;
}

function lineStarts(source) {
	const starts = [0];
	for (let index = 0; index < source.length; index += 1) {
		if (source[index] === "\n") starts.push(index + 1);
	}
	return starts;
}

function positionForOffset(starts, offset) {
	let line = 1;
	for (let index = 0; index < starts.length; index += 1) {
		if (starts[index] > offset) break;
		line = index + 1;
	}
	return { line, column: offset - starts[line - 1] + 1 };
}

function projectFilesFromTsconfig(root, ts) {
	const configPath = path.join(root, "tsconfig.json");
	const packagePath = path.join(root, "package.json");
	if (!ts || !fs.existsSync(configPath) || !fs.existsSync(packagePath)) return null;
	try {
		const read = ts.readConfigFile(configPath, ts.sys.readFile);
		if (read.error) return null;
		const parsed = ts.parseJsonConfigFileContent(read.config, ts.sys, root);
		return parsed.fileNames
			.map((file) => path.relative(root, file).split(path.sep).join("/"))
			.filter((file) => EXTENSIONS.has(path.extname(file)))
			.sort();
	} catch {
		return null;
	}
}

function moduleNameForPath(filePath) {
	const parsed = path.posix.parse(filePath);
	const parts = parsed.dir.split("/").filter(Boolean);
	if (parts[0] === "src") parts.shift();
	return [...parts, parsed.name].join(".");
}

function findMatchingBrace(source, openIndex) {
	let depth = 0;
	let quote = null;
	let lineComment = false;
	let blockComment = false;
	for (let index = openIndex; index < source.length; index += 1) {
		const char = source[index];
		const next = source[index + 1];
		if (lineComment) {
			if (char === "\n") lineComment = false;
			continue;
		}
		if (blockComment) {
			if (char === "*" && next === "/") {
				blockComment = false;
				index += 1;
			}
			continue;
		}
		if (quote) {
			if (char === "\\") {
				index += 1;
				continue;
			}
			if (char === quote) quote = null;
			continue;
		}
		if (char === "/" && next === "/") {
			lineComment = true;
			index += 1;
			continue;
		}
		if (char === "/" && next === "*") {
			blockComment = true;
			index += 1;
			continue;
		}
		if (char === "\"" || char === "'" || char === "`") {
			quote = char;
			continue;
		}
		if (char === "{") depth += 1;
		if (char === "}") {
			depth -= 1;
			if (depth === 0) return index;
		}
	}
	return source.length - 1;
}

function findFunctionBody(source, declarationEnd) {
	const open = source.indexOf("{", declarationEnd);
	if (open < 0) return null;
	return { open, close: findMatchingBrace(source, open) };
}

function makeSymbol(language, filePath, kind, displayName, qualifiedName, startByte, endByte, starts) {
	const start = positionForOffset(starts, startByte);
	const end = positionForOffset(starts, Math.max(startByte, endByte));
	return {
		external_symbol: stableId(language, filePath, kind, qualifiedName, start.line, startByte),
		display_name: displayName,
		kind,
		file_path: filePath,
		start_line: start.line,
		end_line: end.line,
		start_byte: startByte,
		end_byte: endByte,
	};
}

function makeReference(fromExternalSymbol, toExternalSymbol, relationship, filePath, starts, startByte, endByte) {
	const start = positionForOffset(starts, startByte);
	const end = positionForOffset(starts, Math.max(startByte, endByte));
	return {
		from_external_symbol: fromExternalSymbol,
		to_external_symbol: toExternalSymbol,
		relationship,
		file_path: filePath,
		line: start.line,
		column: start.column,
		end_line: end.line,
		end_column: end.column,
		confidence: 0.75,
		provenance: "typescript_source",
	};
}

function collectLocalBindings(bodySource) {
	const bindings = new Set();
	const bindingRe = /\b(?:const|let|var|function|class)\s+([A-Za-z_$][\w$]*)/g;
	let match;
	while ((match = bindingRe.exec(bodySource)) !== null) {
		bindings.add(match[1]);
	}
	const paramRe = /^\s*([A-Za-z_$][\w$]*)\s*(?::|,|\)|=)/;
	const open = bodySource.indexOf("(");
	const close = open >= 0 ? bodySource.indexOf(")", open) : -1;
	if (open >= 0 && close > open) {
		for (const part of bodySource.slice(open + 1, close).split(",")) {
			const param = part.trim().match(paramRe);
			if (param) bindings.add(param[1]);
		}
	}
	return bindings;
}

function collectVariableTypes(bodySource, localBindings, localImports, functionReturnClass) {
	const variableTypes = new Map();
	const assignmentRe = /\b(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*=\s*([A-Za-z_$][\w$]*)\s*\(/g;
	let match;
	while ((match = assignmentRe.exec(bodySource)) !== null) {
		const variable = match[1];
		const callee = match[2];
		if (localBindings.has(callee) && localImports.has(callee)) continue;
		const target = localImports.get(callee) || callee;
		const returned = functionReturnClass.get(target);
		if (returned) variableTypes.set(variable, returned);
	}
	return variableTypes;
}

function parseImportNames(raw) {
	return raw
		.split(",")
		.map((item) => item.trim().replace(/^type\s+/, ""))
		.filter(Boolean)
		.map((item) => {
			const parts = item.split(/\s+as\s+/);
			return { imported: parts[0].trim(), local: (parts[1] || parts[0]).trim() };
		});
}

function resolveRelativeModule(currentFile, specifier, fileSet) {
	if (!specifier.startsWith(".")) return null;
	const base = path.posix.normalize(path.posix.join(path.posix.dirname(currentFile), specifier));
	if (fileSet.has(base)) return base;
	for (const extension of EXTENSIONS) {
		if (fileSet.has(`${base}${extension}`)) return `${base}${extension}`;
	}
	for (const extension of EXTENSIONS) {
		if (fileSet.has(`${base}/index${extension}`)) return `${base}/index${extension}`;
	}
	return null;
}

function addKnown(knownByDisplay, knownByFileAndDisplay, symbol) {
	if (!knownByDisplay.has(symbol.display_name)) knownByDisplay.set(symbol.display_name, []);
	knownByDisplay.get(symbol.display_name).push(symbol);
	const fileKey = `${symbol.file_path}:${symbol.display_name}`;
	if (!knownByFileAndDisplay.has(fileKey)) knownByFileAndDisplay.set(fileKey, symbol);
}

function resolveUniqueDisplay(knownByDisplay, displayName) {
	const matches = knownByDisplay.get(displayName) || [];
	return matches.length === 1 ? matches[0] : null;
}

function enclosingScope(scopes, offset) {
	let best = null;
	for (const scope of scopes) {
		if (scope.bodyStart <= offset && offset <= scope.bodyEnd) {
			if (!best || scope.bodyStart >= best.bodyStart) best = scope;
		}
	}
	return best;
}

function sortedJson(payload) {
	return JSON.stringify(payload, null, 2) + "\n";
}

function collectSymbols(root, files) {
	const symbols = [];
	const fileData = new Map();
	const knownByDisplay = new Map();
	const knownByFileAndDisplay = new Map();

	for (const filePath of files) {
		const absolute = path.join(root, filePath);
		const source = fs.readFileSync(absolute, "utf8");
		const starts = lineStarts(source);
		const moduleName = moduleNameForPath(filePath);
		const moduleSymbol = makeSymbol("typescript", filePath, "module", moduleName, moduleName, 0, Math.max(0, source.length - 1), starts);
		const scopes = [{
			external_symbol: moduleSymbol.external_symbol,
			display_name: moduleSymbol.display_name,
			bodyStart: 0,
			bodyEnd: source.length,
			localBindings: new Set(),
			variableTypes: new Map(),
		}];
		const declarations = new Set();
		const functionBodies = [];

		symbols.push(moduleSymbol);
		addKnown(knownByDisplay, knownByFileAndDisplay, moduleSymbol);

		const classRe = /\b(?:export\s+)?class\s+([A-Za-z_$][\w$]*)/g;
		let match;
		while ((match = classRe.exec(source)) !== null) {
			const name = match[1];
			const nameOffset = match.index + match[0].lastIndexOf(name);
			declarations.add(nameOffset);
			const body = findFunctionBody(source, classRe.lastIndex);
			const endByte = body ? body.close + 1 : classRe.lastIndex;
			const symbol = makeSymbol("typescript", filePath, "class", name, `${moduleName}.${name}`, match.index, endByte, starts);
			symbols.push(symbol);
			addKnown(knownByDisplay, knownByFileAndDisplay, symbol);

			if (!body) continue;
			const methodRe = /\b([A-Za-z_$][\w$]*)\s*\([^)]*\)\s*\{/g;
			const bodySource = source.slice(body.open + 1, body.close);
			let methodMatch;
			while ((methodMatch = methodRe.exec(bodySource)) !== null) {
				const methodName = methodMatch[1];
				if (CALL_KEYWORDS.has(methodName)) continue;
				const methodOffset = body.open + 1 + methodMatch.index;
				declarations.add(methodOffset);
				const methodOpen = body.open + 1 + methodRe.lastIndex - 1;
				const methodClose = findMatchingBrace(source, methodOpen);
				const displayName = `${name}.${methodName}`;
				const methodSymbol = makeSymbol("typescript", filePath, "method", displayName, `${moduleName}.${displayName}`, methodOffset, methodClose + 1, starts);
				symbols.push(methodSymbol);
				addKnown(knownByDisplay, knownByFileAndDisplay, methodSymbol);
				const scopeBody = source.slice(methodOffset, methodClose + 1);
				const localBindings = collectLocalBindings(scopeBody);
				scopes.push({
					external_symbol: methodSymbol.external_symbol,
					display_name: methodSymbol.display_name,
					bodyStart: methodOpen + 1,
					bodyEnd: methodClose,
					localBindings,
					variableTypes: new Map(),
				});
			}
		}

		const functionRe = /\b(?:export\s+)?(?:async\s+)?function\s+([A-Za-z_$][\w$]*)\s*\(/g;
		while ((match = functionRe.exec(source)) !== null) {
			const name = match[1];
			const nameOffset = match.index + match[0].lastIndexOf(name);
			declarations.add(nameOffset);
			const body = findFunctionBody(source, functionRe.lastIndex);
			const endByte = body ? body.close + 1 : functionRe.lastIndex;
			const symbol = makeSymbol("typescript", filePath, "function", name, `${moduleName}.${name}`, match.index, endByte, starts);
			symbols.push(symbol);
			addKnown(knownByDisplay, knownByFileAndDisplay, symbol);
			if (body) {
				const scopeBody = source.slice(match.index, body.close + 1);
				const localBindings = collectLocalBindings(scopeBody);
				scopes.push({
					external_symbol: symbol.external_symbol,
					display_name: symbol.display_name,
					bodyStart: body.open + 1,
					bodyEnd: body.close,
					localBindings,
					variableTypes: new Map(),
				});
				functionBodies.push({ symbol, bodySource: source.slice(body.open + 1, body.close) });
			}
		}

		fileData.set(filePath, { source, starts, scopes, declarations, functionBodies });
	}

	return { symbols, fileData, knownByDisplay, knownByFileAndDisplay };
}

function collectFunctionReturns(files, fileData, knownByDisplay) {
	const functionReturnClass = new Map();
	const returnNewRe = /\breturn\s+new\s+([A-Za-z_$][\w$]*)\s*\(/g;
	for (const filePath of files) {
		for (const { symbol, bodySource } of fileData.get(filePath).functionBodies) {
			let match;
			while ((match = returnNewRe.exec(bodySource)) !== null) {
				const classSymbol = resolveUniqueDisplay(knownByDisplay, match[1]);
				if (classSymbol && classSymbol.kind === "class") {
					functionReturnClass.set(symbol.display_name, classSymbol.display_name);
					functionReturnClass.set(symbol.external_symbol, classSymbol.display_name);
				}
			}
		}
	}
	return functionReturnClass;
}

function collectReferences(files, fileData, knownByDisplay, knownByFileAndDisplay, functionReturnClass) {
	const references = [];
	const fileSet = new Set(files);
	const importsByFile = new Map();

	for (const filePath of files) {
		const { source, starts, scopes } = fileData.get(filePath);
		const moduleScope = scopes[0];
		const localImports = new Map();
		const importRe = /import\s+\{([^}]+)\}\s+from\s+["']([^"']+)["']/g;
		let match;
		while ((match = importRe.exec(source)) !== null) {
			const moduleFile = resolveRelativeModule(filePath, match[2], fileSet);
			if (!moduleFile) continue;
			for (const { imported, local } of parseImportNames(match[1])) {
				const target = knownByFileAndDisplay.get(`${moduleFile}:${imported}`);
				if (!target) continue;
				localImports.set(local, target.external_symbol);
				references.push(makeReference(moduleScope.external_symbol, target.external_symbol, "import", filePath, starts, match.index, importRe.lastIndex));
			}
		}
		importsByFile.set(filePath, localImports);
	}

	for (const filePath of files) {
		const { source, starts, scopes, declarations } = fileData.get(filePath);
		const localImports = importsByFile.get(filePath) || new Map();
		for (const scope of scopes) {
			scope.variableTypes = collectVariableTypes(source.slice(scope.bodyStart, scope.bodyEnd), scope.localBindings, localImports, functionReturnClass);
		}

		const memberCallRe = /\b([A-Za-z_$][\w$]*)\.([A-Za-z_$][\w$]*)\s*\(/g;
		let match;
		while ((match = memberCallRe.exec(source)) !== null) {
			const receiver = match[1];
			const method = match[2];
			const methodOffset = match.index + match[0].lastIndexOf(method);
			const scope = enclosingScope(scopes, match.index) || scopes[0];
			let target = null;
			const receiverType = scope.variableTypes.get(receiver);
			if (receiverType) {
				target = resolveUniqueDisplay(knownByDisplay, `${receiverType}.${method}`);
			} else if (!scope.localBindings.has(receiver)) {
				const candidates = [];
				for (const items of knownByDisplay.values()) {
					for (const item of items) {
						if (item.kind === "method" && item.display_name.endsWith(`.${method}`)) candidates.push(item);
					}
				}
				if (candidates.length === 1) target = candidates[0];
			}
			if (target) {
				references.push(makeReference(scope.external_symbol, target.external_symbol, "call", filePath, starts, methodOffset, memberCallRe.lastIndex));
			}
		}

		const callRe = /\b([A-Za-z_$][\w$]*)\s*\(/g;
		while ((match = callRe.exec(source)) !== null) {
			const name = match[1];
			if (CALL_KEYWORDS.has(name)) continue;
			const nameOffset = match.index;
			if (declarations.has(nameOffset)) continue;
			if (nameOffset > 0 && source[nameOffset - 1] === ".") continue;
			if (nameOffset > 0 && /[A-Za-z_$\w$]/.test(source[nameOffset - 1])) continue;
			const before = source.slice(Math.max(0, nameOffset - 16), nameOffset);
			if (/\b(new|function|class)\s+$/.test(before)) {
				if (!/\bnew\s+$/.test(before)) continue;
			}
			const scope = enclosingScope(scopes, nameOffset) || scopes[0];
			if (scope.localBindings.has(name) && localImports.has(name)) continue;

			let target = null;
			const imported = localImports.get(name);
			if (imported) {
				for (const items of knownByDisplay.values()) {
					target = items.find((item) => item.external_symbol === imported) || target;
					if (target) break;
				}
			} else if (!scope.localBindings.has(name) || BINDING_WORDS.has(name)) {
				target = resolveUniqueDisplay(knownByDisplay, name);
			}
			if (!target) continue;
			references.push(makeReference(scope.external_symbol, target.external_symbol, "call", filePath, starts, nameOffset, callRe.lastIndex));
		}
	}

	return references;
}

function main(argv) {
	if (argv.length !== 3 || argv[0] !== "index" || argv[1] !== "--output" || !argv[2]) {
		return usage();
	}
	const root = process.cwd();
	const ts = loadProjectTypescript(root);
	const files = projectFilesFromTsconfig(root, ts) || discoverFiles(root);
	if (files.length === 0) {
		console.error("no TypeScript or JavaScript files found");
		return 69;
	}

	const collected = collectSymbols(root, files);
	const functionReturnClass = collectFunctionReturns(files, collected.fileData, collected.knownByDisplay);
	const references = collectReferences(
		files,
		collected.fileData,
		collected.knownByDisplay,
		collected.knownByFileAndDisplay,
		functionReturnClass,
	);

	const symbols = collected.symbols.sort((left, right) =>
		(left.file_path || "").localeCompare(right.file_path || "") ||
		(left.start_line || 0) - (right.start_line || 0) ||
		left.external_symbol.localeCompare(right.external_symbol)
	);
	references.sort((left, right) =>
		left.file_path.localeCompare(right.file_path) ||
		left.line - right.line ||
		(left.column || 0) - (right.column || 0) ||
		left.relationship.localeCompare(right.relationship) ||
		(left.to_external_symbol || "").localeCompare(right.to_external_symbol || "")
	);

	const payload = {
		source_kind: "typescript_source",
		producer: PRODUCER,
		language: "typescript",
		root_path: root,
		symbols,
		references,
	};
	fs.mkdirSync(path.dirname(path.resolve(argv[2])), { recursive: true });
	fs.writeFileSync(argv[2], sortedJson(payload), "utf8");
	return 0;
}

if (require.main === module) {
	process.exitCode = main(process.argv.slice(2));
}
