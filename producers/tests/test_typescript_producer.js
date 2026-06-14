const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { spawnSync } = require("node:child_process");
const test = require("node:test");

const ROOT = path.resolve(__dirname, "..", "..");
const FIXTURE = path.join(ROOT, "producers", "tests", "fixtures", "typescript");
const PRODUCER = path.join(ROOT, "producers", "typescript", "index.js");
const WRAPPER = path.join(ROOT, "producers", "bin", "code-intelligence-external-typescript");

function runProducer(cwd = FIXTURE) {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "typescript-producer-"));
  const output = path.join(tmp, "typescript-normalized.json");
  const result = spawnSync(
    process.execPath,
    [PRODUCER, "index", "--output", output],
    { cwd, encoding: "utf8" },
  );
  assert.equal(result.status, 0, result.stderr);
  return JSON.parse(fs.readFileSync(output, "utf8"));
}

function writeFixture(root, files) {
  for (const [relative, source] of Object.entries(files)) {
    const file = path.join(root, relative);
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, source, "utf8");
  }
}

test("emits TypeScript symbols, imports, and calls", () => {
  const payload = runProducer();
  const symbols = new Map(payload.symbols.map((item) => [item.display_name, item]));
  assert.equal(payload.source_kind, "typescript_source");
  assert.equal(payload.language, "typescript");
  assert.ok(symbols.has("UserService"));
  assert.ok(symbols.has("UserService.load"));
  assert.ok(symbols.has("makeService"));
  assert.ok(symbols.has("renderUser"));

  assert.ok(payload.references.every((item) => ["call", "import"].includes(item.relationship)));
  const relationships = new Set(
    payload.references.map((item) => `${item.relationship}:${item.to_external_symbol}`),
  );
  assert.ok(relationships.has(`import:${symbols.get("makeService").external_symbol}`));
  assert.ok(relationships.has(`call:${symbols.get("makeService").external_symbol}`));
  assert.ok(relationships.has(`call:${symbols.get("UserService.load").external_symbol}`));
  assert.ok(
    payload.references.some(
      (item) =>
        item.relationship === "call" &&
        item.to_external_symbol === symbols.get("makeService").external_symbol &&
        item.from_external_symbol === symbols.get("renderUser").external_symbol,
    ),
  );
  assert.ok(
    payload.references.some(
      (item) =>
        item.relationship === "call" &&
        item.to_external_symbol === symbols.get("UserService.load").external_symbol &&
        item.from_external_symbol === symbols.get("renderUser").external_symbol,
    ),
  );
  assert.ok(
    payload.references.some(
      (item) =>
        item.relationship === "import" &&
        item.to_external_symbol === symbols.get("makeService").external_symbol &&
        item.file_path === "src/app.ts" &&
        item.line === 1 &&
        item.column === 1,
    ),
  );

  const serviceSource = fs.readFileSync(path.join(FIXTURE, "src", "service.ts"), "utf8");
  const expectedStart = serviceSource.indexOf("export function makeService");
  assert.equal(symbols.get("makeService").start_byte, expectedStart);
  assert.ok(symbols.get("makeService").start_byte > 0);
});

test("output is deterministic", () => {
  assert.deepEqual(runProducer(), runProducer());
});

test("local bindings shadow imported names inside the same function", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "typescript-shadow-"));
  writeFixture(root, {
    "package.json": "{\"type\":\"module\"}\n",
    "src/service.ts": "export function makeService() {\n  return {};\n}\n",
    "src/app.ts": [
      "import { makeService } from \"./service\";",
      "",
      "export function renderUser(id: string) {",
      "  const makeService = () => ({ load(value: string) { return value; } });",
      "  const service = makeService();",
      "  return service.load(id);",
      "}",
      "",
    ].join("\n"),
  });

  const payload = runProducer(root);
  const symbols = new Map(payload.symbols.map((item) => [item.display_name, item]));
  assert.ok(!payload.references.some(
    (item) =>
      item.relationship === "call" &&
      item.to_external_symbol === symbols.get("makeService").external_symbol,
  ));
});

test("unresolved import bindings do not fall back to same-name local symbols", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "typescript-unresolved-imports-"));
  writeFixture(root, {
    "package.json": "{\"type\":\"module\"}\n",
    "src/local.ts": [
      "export function retry() { return 1; }",
      "export function aliasRetry() { return 2; }",
      "export function runDefault() { return 3; }",
      "export function missingRelative() { return 4; }",
      "export function tracked() { return 5; }",
      "export class Retrier { retry() { return 6; } }",
      "",
    ].join("\n"),
    "src/tracked.ts": "export function tracked() { return 7; }\n",
    "src/app.ts": [
      "import runDefault, { retry, retry as aliasRetry } from \"p-retry\";",
      "import * as retryTools from \"retry-tools\";",
      "import { missingRelative } from \"./missing\";",
      "import { tracked } from \"./tracked\";",
      "",
      "export function useImports() {",
      "  retry();",
      "  aliasRetry();",
      "  runDefault();",
      "  retryTools.retry();",
      "  missingRelative();",
      "  tracked();",
      "}",
      "",
    ].join("\n"),
  });

  const payload = runProducer(root);
  const symbols = new Map(payload.symbols.map((item) => [item.display_name, item]));
  const localNames = ["retry", "aliasRetry", "runDefault", "missingRelative"];
  for (const name of localNames) {
    assert.ok(!payload.references.some(
      (item) =>
        item.relationship === "call" &&
        item.to_external_symbol === symbols.get(name).external_symbol,
    ));
  }
  assert.ok(!payload.references.some(
    (item) =>
      item.relationship === "call" &&
      item.to_external_symbol === symbols.get("Retrier.retry").external_symbol,
  ));
  assert.ok(payload.references.some(
    (item) =>
      item.relationship === "call" &&
      item.to_external_symbol === symbols.get("tracked").external_symbol,
  ));
  assert.ok(payload.references.some(
    (item) =>
      item.relationship === "import" &&
      item.to_external_symbol === symbols.get("tracked").external_symbol,
  ));
  assert.equal(
    payload.references.filter((item) => item.relationship === "import").length,
    1,
  );
});

test("generated directories are not indexed", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "typescript-generated-"));
  writeFixture(root, {
    "package.json": "{\"type\":\"module\"}\n",
    "src/app.ts": "export function kept() {\n  return 1;\n}\n",
    "generated/generated.ts": "export function generatedOnly() {\n  return 2;\n}\n",
  });

  const payload = runProducer(root);
  const displayNames = payload.symbols.map((item) => item.display_name);
  assert.ok(displayNames.includes("kept"));
  assert.ok(!displayNames.includes("generatedOnly"));
  assert.ok(!payload.symbols.some((item) => item.file_path.startsWith("generated/")));
});

test("symbol byte spans use UTF-8 offsets", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "typescript-utf8-"));
  writeFixture(root, {
    "package.json": "{\"type\":\"module\"}\n",
    "src/utf8.ts": "// éé\nexport function marker() {}\n",
  });

  const payload = runProducer(root);
  const marker = payload.symbols.find((item) => item.display_name === "marker");
  assert.equal(marker.start_byte, 8);
  assert.equal(marker.external_symbol, "typescript:src/utf8.ts:function:utf8.marker:2:8");
  assert.ok(marker.end_byte > marker.start_byte);
});

// Task 7 owns packaging support-file copying checks; Task 3 stays focused on source producer behavior.
test("calls inside comments and strings are ignored", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "typescript-masked-calls-"));
  writeFixture(root, {
    "package.json": "{\"type\":\"module\"}\n",
    "src/app.ts": [
      "export function target() {",
      "  return 1;",
      "}",
      "",
      "export function references() {",
      "  // target();",
      "  /* target(); */",
      "  const single = 'target()';",
      "  const double = \"target()\";",
      "  const templated = `target()`;",
      "  return single + double + templated;",
      "}",
      "",
    ].join("\n"),
  });

  const payload = runProducer(root);
  const symbols = new Map(payload.symbols.map((item) => [item.display_name, item]));
  assert.ok(!payload.references.some(
    (item) =>
      item.relationship === "call" &&
      item.to_external_symbol === symbols.get("target").external_symbol,
  ));
});

test("object-literal method shorthand declarations are not emitted as calls", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "typescript-object-method-"));
  writeFixture(root, {
    "package.json": "{\"type\":\"module\"}\n",
    "src/app.ts": [
      "export function save() {",
      "  return 1;",
      "}",
      "",
      "export function load() {",
      "  return 3;",
      "}",
      "",
      "export function makeActions() {",
      "  const actions = {",
      "    save() {",
      "      return 2;",
      "    },",
      "    async saveLater() {",
      "      return save();",
      "    },",
      "    async save() {",
      "      return 4;",
      "    },",
      "    *load() {",
      "      yield 5;",
      "    }",
      "  };",
      "  return actions;",
      "}",
      "",
    ].join("\n"),
  });

  const payload = runProducer(root);
  const symbols = new Map(payload.symbols.map((item) => [item.display_name, item]));
  const calls = payload.references.filter((item) => item.relationship === "call");
  assert.ok(!calls.some(
    (item) => item.line === 11 && item.to_external_symbol === symbols.get("save").external_symbol,
  ));
  assert.ok(calls.some(
    (item) => item.line === 15 && item.to_external_symbol === symbols.get("save").external_symbol,
  ));
  assert.ok(!calls.some(
    (item) => item.line === 17 && item.to_external_symbol === symbols.get("save").external_symbol,
  ));
  assert.ok(!calls.some(
    (item) => item.line === 20 && item.to_external_symbol === symbols.get("load").external_symbol,
  ));
});

test("imports inside comments do not create import edges", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "typescript-commented-import-"));
  writeFixture(root, {
    "package.json": "{\"type\":\"module\"}\n",
    "src/service.ts": "export function makeService() {\n  return {};\n}\n",
    "src/app.ts": [
      "// import { makeService } from \"./service\";",
      "export function renderUser() {",
      "  return 1;",
      "}",
      "",
    ].join("\n"),
  });

  const payload = runProducer(root);
  assert.ok(!payload.references.some((item) => item.relationship === "import"));
});

test("function declarations inside comments and strings are ignored", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "typescript-masked-declarations-"));
  writeFixture(root, {
    "package.json": "{\"type\":\"module\"}\n",
    "src/app.ts": [
      "// export function ghost() { return 1; }",
      "const source = 'export function stringGhost() { return 2; }';",
      "export function real() {",
      "  return source;",
      "}",
      "",
    ].join("\n"),
  });

  const payload = runProducer(root);
  const displayNames = payload.symbols.map((item) => item.display_name);
  assert.ok(displayNames.includes("real"));
  assert.ok(!displayNames.includes("ghost"));
  assert.ok(!displayNames.includes("stringGhost"));
});

test("nested function declarations are not emitted as module-level symbols", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "typescript-nested-functions-"));
  writeFixture(root, {
    "package.json": "{\"type\":\"module\"}\n",
    "src/app.ts": [
      "export function outer() {",
      "  function inner() {",
      "    return 1;",
      "  }",
      "  return inner();",
      "}",
      "",
    ].join("\n"),
  });

  const payload = runProducer(root);
  const displayNames = payload.symbols.map((item) => item.display_name);
  assert.ok(displayNames.includes("outer"));
  assert.ok(!displayNames.includes("inner"));
});

test("return-new inference ignores comments and strings", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "typescript-masked-return-new-"));
  writeFixture(root, {
    "package.json": "{\"type\":\"module\"}\n",
    "src/service.ts": [
      "export class UserService {",
      "  load() {",
      "    return 1;",
      "  }",
      "}",
      "",
      "export function makeService() {",
      "  // return new UserService();",
      "  const note = 'return new UserService()';",
      "  return {};",
      "}",
      "",
      "export function renderUser() {",
      "  const service = makeService();",
      "  return service.load();",
      "}",
      "",
    ].join("\n"),
  });

  const payload = runProducer(root);
  const symbols = new Map(payload.symbols.map((item) => [item.display_name, item]));
  assert.ok(!payload.references.some(
    (item) =>
      item.relationship === "call" &&
      item.to_external_symbol === symbols.get("UserService.load").external_symbol,
  ));
});

test("class method extraction ignores nested method-shaped declarations", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "typescript-class-depth-"));
  writeFixture(root, {
    "package.json": "{\"type\":\"module\"}\n",
    "src/service.ts": [
      "export class Container {",
      "  run() {",
      "    const helper = {",
      "      fake() {",
      "        return 1;",
      "      }",
      "    };",
      "    function nested() {",
      "      return helper.fake();",
      "    }",
      "    return nested();",
      "  }",
      "",
      "  top() {",
      "    return 2;",
      "  }",
      "}",
      "",
    ].join("\n"),
  });

  const payload = runProducer(root);
  const displayNames = payload.symbols.map((item) => item.display_name);
  assert.ok(displayNames.includes("Container"));
  assert.ok(displayNames.includes("Container.run"));
  assert.ok(displayNames.includes("Container.top"));
  assert.ok(!displayNames.includes("Container.fake"));
  assert.ok(!displayNames.includes("Container.nested"));
});

test("wrapper usage and missing output errors exit 64", () => {
  const wrongCommand = spawnSync(WRAPPER, ["wrong"], { cwd: FIXTURE, encoding: "utf8" });
  assert.equal(wrongCommand.status, 64);
  assert.match(wrongCommand.stderr, /usage:/);

  const missingOutput = spawnSync(WRAPPER, ["index"], { cwd: FIXTURE, encoding: "utf8" });
  assert.equal(missingOutput.status, 64);
  assert.match(missingOutput.stderr, /usage:/);
});

test("output write errors exit 64 without stack trace", () => {
  const outputDirectory = fs.mkdtempSync(path.join(os.tmpdir(), "typescript-output-dir-"));
  const result = spawnSync(
    process.execPath,
    [PRODUCER, "index", "--output", outputDirectory],
    { cwd: FIXTURE, encoding: "utf8" },
  );
  assert.equal(result.status, 64);
  assert.match(result.stderr, /failed to write output:/);
  assert.doesNotMatch(result.stderr, /at .*index\.js/);
});

test("no TypeScript or JavaScript files exits 69", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "typescript-empty-"));
  fs.writeFileSync(path.join(root, "package.json"), "{\"type\":\"module\"}\n", "utf8");
  const output = path.join(root, "normalized.json");
  const result = spawnSync(
    process.execPath,
    [PRODUCER, "index", "--output", output],
    { cwd: root, encoding: "utf8" },
  );
  assert.equal(result.status, 69);
  assert.match(result.stderr, /no TypeScript or JavaScript files found/);
});
