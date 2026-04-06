#!/usr/bin/env node

const { spawnSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const pkg = require("../package.json");
const REPO = "ysm-dev/domaingrep";

function releaseTarget() {
  if (process.platform === "darwin") {
    if (process.arch === "arm64") {
      return { target: "aarch64-apple-darwin", executable: "domaingrep", archiveExt: "tar.gz" };
    }
    if (process.arch === "x64") {
      return { target: "x86_64-apple-darwin", executable: "domaingrep", archiveExt: "tar.gz" };
    }
  }

  if (process.platform === "linux") {
    const libc = detectLinuxLibc();
    if (process.arch === "arm64") {
      return {
        target: libc === "gnu" ? "aarch64-unknown-linux-gnu" : "aarch64-unknown-linux-musl",
        executable: "domaingrep",
        archiveExt: "tar.gz",
      };
    }
    if (process.arch === "x64") {
      return {
        target: libc === "gnu" ? "x86_64-unknown-linux-gnu" : "x86_64-unknown-linux-musl",
        executable: "domaingrep",
        archiveExt: "tar.gz",
      };
    }
  }

  if (process.platform === "win32" && process.arch === "x64") {
    return { target: "x86_64-pc-windows-msvc", executable: "domaingrep.exe", archiveExt: "zip" };
  }

  return null;
}

function detectLinuxLibc() {
  if (process.platform !== "linux") {
    return null;
  }

  if (process.report && typeof process.report.getReport === "function") {
    const report = process.report.getReport();
    if (report && report.header && report.header.glibcVersionRuntime) {
      return "gnu";
    }
  }

  return "musl";
}

function cacheRoot() {
  if (process.env.DOMAINGREP_NPM_BIN_DIR) {
    return process.env.DOMAINGREP_NPM_BIN_DIR;
  }

  if (process.platform === "win32" && process.env.LOCALAPPDATA) {
    return path.join(process.env.LOCALAPPDATA, "domaingrep", "npm");
  }

  return path.join(os.homedir(), ".domaingrep", "npm");
}

function resolveBinary() {
  const targetInfo = releaseTarget();
  if (!targetInfo) {
    console.error(
      `error: unsupported platform ${process.platform}/${process.arch} for npm distribution`
    );
    process.exit(1);
  }

  const cachedBinary = path.join(cacheRoot(), pkg.version, targetInfo.target, targetInfo.executable);
  if (fs.existsSync(cachedBinary)) {
    return cachedBinary;
  }

  try {
    return ensureDownloadedBinary(targetInfo, cachedBinary);
  } catch (error) {
    console.error(`error: failed to prepare domaingrep binary: ${error.message}`);
    console.error("  = help: check your network connection or use cargo/homebrew instead");
    process.exit(1);
  }
}

function ensureDownloadedBinary(targetInfo, binaryPath) {
  fs.mkdirSync(path.dirname(binaryPath), { recursive: true });

  const versionTag = `v${pkg.version}`;
  const archiveName = `domaingrep-${targetInfo.target}.${targetInfo.archiveExt}`;
  const archivePath = path.join(path.dirname(binaryPath), archiveName);
  const assetUrl = `https://github.com/${REPO}/releases/download/${versionTag}/${archiveName}`;

  if (!fs.existsSync(archivePath)) {
    downloadToFile(assetUrl, archivePath);
  }

  extractArchive(archivePath, path.dirname(binaryPath), targetInfo.archiveExt);

  if (!fs.existsSync(binaryPath)) {
    throw new Error(`binary was not extracted from ${archiveName}`);
  }

  if (process.platform !== "win32") {
    fs.chmodSync(binaryPath, 0o755);
  }

  return binaryPath;
}

function downloadToFile(url, destination) {
  const result = spawnSync("curl", ["-fL", url, "-o", destination], { stdio: "inherit" });
  if (result.status !== 0) {
    throw new Error("failed to download release asset");
  }
}

function extractArchive(archivePath, directory, archiveExt) {
  if (archiveExt === "zip") {
    const result = spawnSync(
      "powershell.exe",
      [
        "-NoProfile",
        "-Command",
        `Expand-Archive -LiteralPath '${archivePath.replace(/'/g, "''")}' -DestinationPath '${directory.replace(/'/g, "''")}' -Force`,
      ],
      { stdio: "inherit" }
    );
    if (result.status !== 0) {
      throw new Error("failed to extract zip archive");
    }
    return;
  }

  const result = spawnSync("tar", ["-xzf", archivePath, "-C", directory], { stdio: "inherit" });
  if (result.status !== 0) {
    throw new Error("failed to extract tar.gz archive");
  }
}

const binaryPath = resolveBinary();
const result = spawnSync(binaryPath, process.argv.slice(2), { stdio: "inherit" });

if (result.error) {
  console.error(`error: failed to execute domaingrep binary: ${result.error.message}`);
  process.exit(1);
}

process.exit(result.status === null ? 1 : result.status);
