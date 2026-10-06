import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import {
  cargoVersion,
  check,
  collect,
  FILES,
  lockVersions,
  setCargoVersion,
  tauriVersion,
  versionOfTag,
} from "./version.mjs";

const CARGO = `[workspace]
members = ["packages/core", "apps/desktop/src-tauri"]

[workspace.package]
version = "0.1.0"
edition = "2024"

[workspace.dependencies]
serde = { version = "1" }
`;

test("tags become versions", () => {
  assert.equal(versionOfTag("v0.2.0"), "0.2.0");
  assert.equal(versionOfTag("refs/tags/v1.10.3-beta.1"), "1.10.3-beta.1");
  assert.equal(versionOfTag("0.2.0"), "0.2.0");
  assert.equal(versionOfTag("release-2"), null);
});

test("only the workspace version changes in Cargo.toml", () => {
  assert.equal(cargoVersion(CARGO), "0.1.0");
  const updated = setCargoVersion(CARGO, "0.2.0");
  assert.equal(cargoVersion(updated), "0.2.0");
  assert.match(updated, /serde = \{ version = "1" \}/, "dependencies keep their versions");
});

test("the lock is read only for the workspace crates", () => {
  const lock = `
[[package]]
name = "orchestrator-core"
version = "0.1.0"

[[package]]
name = "serde"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
`;
  assert.deepEqual(lockVersions(lock, ["orchestrator-core", "serde"]), { "orchestrator-core": "0.1.0" });
});

test("the Tauri version can point to a package.json", () => {
  assert.equal(tauriVersion({ version: "1.2.3" }, () => ({})), "1.2.3");
  assert.equal(tauriVersion({ version: "../package.json" }, () => ({ version: "1.2.4" })), "1.2.4");
});

test("check names every place that differs, and the tag", () => {
  const found = { [FILES.root]: "0.2.0", [FILES.desktop]: "0.2.0", [FILES.cargo]: "0.1.0" };
  assert.deepEqual(check(found), ["Cargo.toml: 0.1.0 ≠ 0.2.0"]);
  found[FILES.cargo] = "0.2.0";
  assert.deepEqual(check(found), []);
  assert.deepEqual(check(found, "v0.2.0"), []);
  assert.deepEqual(check(found, "v0.3.0"), ["a tag v0.3.0 não bate com a versão 0.2.0"]);
  assert.deepEqual(check(found, "nightly"), ['a tag "nightly" não é uma versão (vX.Y.Z)']);
});

test("collect reads a repository laid out like this one", () => {
  const root = mkdtempSync(join(tmpdir(), "version-"));
  const write = (file, text) => {
    mkdirSync(join(root, file, ".."), { recursive: true });
    writeFileSync(join(root, file), text);
  };
  write(FILES.root, JSON.stringify({ version: "0.3.0" }));
  write(FILES.desktop, JSON.stringify({ version: "0.3.0" }));
  write(FILES.cargo, CARGO.replace('version = "0.1.0"', 'version = "0.3.0"'));
  write(FILES.tauri, JSON.stringify({ version: "../package.json" }));
  write("packages/core/Cargo.toml", '[package]\nname = "orchestrator-core"\nversion.workspace = true\n');
  write("apps/desktop/src-tauri/Cargo.toml", '[package]\nname = "orchestrator-desktop"\nversion.workspace = true\n');
  write(
    FILES.lock,
    '[[package]]\nname = "orchestrator-core"\nversion = "0.3.0"\n\n[[package]]\nname = "orchestrator-desktop"\nversion = "0.2.0"\n',
  );
  const found = collect(root);
  assert.equal(found[FILES.tauri], "0.3.0");
  assert.deepEqual(check(found), ["Cargo.lock (orchestrator-desktop): 0.2.0 ≠ 0.3.0"]);
  assert.equal(readFileSync(join(root, FILES.root), "utf8").includes("0.3.0"), true);
});
