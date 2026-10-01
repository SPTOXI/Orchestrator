#!/usr/bin/env node
// One product version (ADR-0019). The root package.json holds it; the
// desktop package.json, the Cargo workspace and the Tauri configuration
// must say the same.
//
//   node scripts/version.mjs 0.2.0         set the version everywhere
//   node scripts/version.mjs --check       fail if any place differs
//   node scripts/version.mjs --check v0.2.0  …or differs from the tag

import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const SEMVER = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/;

/** Where the version lives, relative to the repository root. */
export const FILES = {
  root: "package.json",
  desktop: "apps/desktop/package.json",
  cargo: "Cargo.toml",
  tauri: "apps/desktop/src-tauri/tauri.conf.json",
  lock: "Cargo.lock",
};

export function isVersion(text) {
  return SEMVER.test(text);
}

/** "v0.2.0" → "0.2.0"; anything that is not a version tag → null. */
export function versionOfTag(tag) {
  const version = tag.replace(/^refs\/tags\//, "").replace(/^v/, "");
  return isVersion(version) ? version : null;
}

/** The `version` of `[workspace.package]` in a Cargo.toml. */
export function cargoVersion(toml) {
  const section = toml.match(/^\[workspace\.package\][^[]*/m);
  const line = section?.[0].match(/^version\s*=\s*"([^"]*)"/m);
  return line ? line[1] : null;
}

export function setCargoVersion(toml, version) {
  const section = toml.match(/^\[workspace\.package\][^[]*/m);
  if (!section) throw new Error("Cargo.toml sem [workspace.package]");
  const updated = section[0].replace(/^version\s*=\s*"[^"]*"/m, `version = "${version}"`);
  return toml.replace(section[0], updated);
}

/** Versions the Cargo.lock records for the workspace crates. */
export function lockVersions(lock, names) {
  const found = {};
  for (const block of lock.split("[[package]]")) {
    const name = block.match(/^name = "([^"]*)"/m)?.[1];
    const version = block.match(/^version = "([^"]*)"/m)?.[1];
    const external = /^source = /m.test(block);
    if (name && version && !external && names.includes(name)) found[name] = version;
  }
  return found;
}

/** The Tauri `version` is either a version or a path to a package.json. */
export function tauriVersion(config, readJson) {
  const value = config.version;
  if (typeof value !== "string") return null;
  return isVersion(value) ? value : readJson(value).version;
}

/** Every place and the version it holds. */
export function collect(root) {
  const read = (file) => readFileSync(join(root, file), "utf8");
  const json = (file) => JSON.parse(read(file));
  const tauriDir = dirname(FILES.tauri);
  const cargo = read(FILES.cargo);
  const members = workspaceCrates(root);
  const found = {
    [FILES.root]: json(FILES.root).version,
    [FILES.desktop]: json(FILES.desktop).version,
    [FILES.cargo]: cargoVersion(cargo),
    [FILES.tauri]: tauriVersion(json(FILES.tauri), (path) => json(join(tauriDir, path))),
  };
  for (const [name, version] of Object.entries(lockVersions(read(FILES.lock), members))) {
    found[`${FILES.lock} (${name})`] = version;
  }
  return found;
}

/** Names of the crates that inherit the workspace version. */
function workspaceCrates(root) {
  const cargo = readFileSync(join(root, FILES.cargo), "utf8");
  const members = cargo.match(/members\s*=\s*\[([^\]]*)\]/)?.[1] ?? "";
  return [...members.matchAll(/"([^"]+)"/g)]
    .map(([, dir]) => readFileSync(join(root, dir, "Cargo.toml"), "utf8"))
    .filter((toml) => /^version\.workspace\s*=\s*true/m.test(toml))
    .map((toml) => toml.match(/^name\s*=\s*"([^"]*)"/m)?.[1])
    .filter(Boolean);
}

/** Problems found, in words; empty when every place agrees (and with the tag). */
export function check(found, tag) {
  const expected = found[FILES.root];
  const problems = [];
  if (!isVersion(expected ?? "")) problems.push(`${FILES.root}: versão inválida "${expected}"`);
  for (const [place, version] of Object.entries(found)) {
    if (version !== expected) problems.push(`${place}: ${version ?? "(sem versão)"} ≠ ${expected}`);
  }
  if (tag !== undefined) {
    const fromTag = versionOfTag(tag);
    if (!fromTag) problems.push(`a tag "${tag}" não é uma versão (vX.Y.Z)`);
    else if (fromTag !== expected) problems.push(`a tag ${tag} não bate com a versão ${expected}`);
  }
  return problems;
}

export function set(root, version) {
  if (!isVersion(version)) throw new Error(`versão inválida: "${version}" (use X.Y.Z)`);
  for (const file of [FILES.root, FILES.desktop]) {
    const path = join(root, file);
    const pkg = JSON.parse(readFileSync(path, "utf8"));
    pkg.version = version;
    writeFileSync(path, `${JSON.stringify(pkg, null, 2)}\n`);
  }
  const cargoPath = join(root, FILES.cargo);
  writeFileSync(cargoPath, setCargoVersion(readFileSync(cargoPath, "utf8"), version));
  const tauriPath = join(root, FILES.tauri);
  const tauri = JSON.parse(readFileSync(tauriPath, "utf8"));
  if (isVersion(tauri.version ?? "")) {
    tauri.version = version;
    writeFileSync(tauriPath, `${JSON.stringify(tauri, null, 2)}\n`);
  }
  // The lock records the workspace crates' versions; only those change.
  execFileSync("cargo", ["update", "--workspace", "--offline"], { cwd: root, stdio: "inherit" });
}

function main(args) {
  const root = join(dirname(fileURLToPath(import.meta.url)), "..");
  if (args[0] === "--check") {
    const problems = check(collect(root), args[1]);
    if (problems.length) {
      console.error(`Versões divergentes:\n  ${problems.join("\n  ")}`);
      process.exit(1);
    }
    console.log(`Versão ${collect(root)[FILES.root]} em todos os lugares${args[1] ? ` e na tag ${args[1]}` : ""}.`);
    return;
  }
  if (args.length !== 1) {
    console.error("Uso: node scripts/version.mjs <X.Y.Z> | --check [vX.Y.Z]");
    process.exit(2);
  }
  set(root, args[0]);
  console.log(`Versão ${args[0]} gravada. Confira com: node scripts/version.mjs --check`);
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  main(process.argv.slice(2));
}
