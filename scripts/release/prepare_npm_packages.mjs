#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const repoRoot = path.resolve(__dirname, "..", "..");

const [version, artifactsDir, outputDir] = process.argv.slice(2);

if (!version || !artifactsDir || !outputDir) {
  console.error("usage: prepare_npm_packages.mjs <version> <artifacts-dir> <output-dir>");
  process.exit(1);
}

const platforms = [
  { slug: "darwin-arm64", target: "aarch64-apple-darwin", binary: "domaingrep" },
  { slug: "darwin-x64", target: "x86_64-apple-darwin", binary: "domaingrep" },
  { slug: "linux-arm64-gnu", target: "aarch64-unknown-linux-gnu", binary: "domaingrep" },
  { slug: "linux-arm64-musl", target: "aarch64-unknown-linux-musl", binary: "domaingrep" },
  { slug: "linux-x64-gnu", target: "x86_64-unknown-linux-gnu", binary: "domaingrep" },
  { slug: "linux-x64-musl", target: "x86_64-unknown-linux-musl", binary: "domaingrep" },
  { slug: "win32-x64", target: "x86_64-pc-windows-msvc", binary: "domaingrep.exe" }
];

resetDir(outputDir);

const mainPackageDir = path.join(outputDir, "domaingrep");
fs.mkdirSync(path.join(mainPackageDir, "bin"), { recursive: true });
copyFile(path.join(repoRoot, "bin", "domaingrep.js"), path.join(mainPackageDir, "bin", "domaingrep.js"));
copyIfExists(path.join(repoRoot, "README.md"), path.join(mainPackageDir, "README.md"));

const rootPackage = readJson(path.join(repoRoot, "package.json"));
rootPackage.version = version;
for (const dependencyName of Object.keys(rootPackage.optionalDependencies ?? {})) {
  rootPackage.optionalDependencies[dependencyName] = version;
}
writeJson(path.join(mainPackageDir, "package.json"), rootPackage);

for (const platform of platforms) {
  const stageDir = path.join(outputDir, "packages", platform.slug);
  fs.mkdirSync(stageDir, { recursive: true });

  const manifest = readJson(path.join(repoRoot, "packages", platform.slug, "package.json"));
  manifest.version = version;
  writeJson(path.join(stageDir, "package.json"), manifest);

  const binarySource = findBinary(artifactsDir, platform.target, platform.binary);
  if (!binarySource) {
    console.error(`missing binary for target ${platform.target}`);
    process.exit(1);
  }

  const binaryDestination = path.join(stageDir, platform.binary);
  copyFile(binarySource, binaryDestination);
  if (platform.binary === "domaingrep") {
    fs.chmodSync(binaryDestination, 0o755);
  }
}

function resetDir(dir) {
  fs.rmSync(dir, { recursive: true, force: true });
  fs.mkdirSync(dir, { recursive: true });
}

function findBinary(root, target, binaryName) {
  for (const entry of walk(root)) {
    if (!entry.endsWith(path.sep + binaryName)) {
      continue;
    }

    if (entry.includes(`${path.sep}${target}${path.sep}`)) {
      return entry;
    }
  }

  return null;
}

function* walk(dir) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const entryPath = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      yield* walk(entryPath);
    } else {
      yield entryPath;
    }
  }
}

function copyFile(source, destination) {
  fs.mkdirSync(path.dirname(destination), { recursive: true });
  fs.copyFileSync(source, destination);
}

function copyIfExists(source, destination) {
  if (fs.existsSync(source)) {
    copyFile(source, destination);
  }
}

function readJson(filePath) {
  return JSON.parse(fs.readFileSync(filePath, "utf8"));
}

function writeJson(filePath, value) {
  fs.writeFileSync(filePath, `${JSON.stringify(value, null, 2)}\n`);
}
