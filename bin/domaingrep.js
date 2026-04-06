#!/usr/bin/env node

const { spawnSync } = require("node:child_process");

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

function resolvePackageName() {
  if (process.platform === "darwin") {
    if (process.arch === "arm64") {
      return "domaingrep-darwin-arm64";
    }
    if (process.arch === "x64") {
      return "domaingrep-darwin-x64";
    }
  }

  if (process.platform === "linux") {
    const libc = detectLinuxLibc();
    if (process.arch === "arm64") {
      return libc === "gnu"
        ? "domaingrep-linux-arm64-gnu"
        : "domaingrep-linux-arm64-musl";
    }
    if (process.arch === "x64") {
      return libc === "gnu"
        ? "domaingrep-linux-x64-gnu"
        : "domaingrep-linux-x64-musl";
    }
  }

  if (process.platform === "win32" && process.arch === "x64") {
    return "domaingrep-win32-x64";
  }

  return null;
}

function resolveBinary() {
  const packageName = resolvePackageName();
  if (!packageName) {
    console.error(
      `error: unsupported platform ${process.platform}/${process.arch} for npm distribution`
    );
    process.exit(1);
  }

  const executable = process.platform === "win32" ? "domaingrep.exe" : "domaingrep";

  try {
    return require.resolve(`${packageName}/${executable}`);
  } catch (error) {
    console.error(`error: failed to locate platform package '${packageName}'`);
    console.error("  = help: reinstall the package or use cargo/homebrew instead");
    process.exit(1);
  }
}

const binaryPath = resolveBinary();
const result = spawnSync(binaryPath, process.argv.slice(2), { stdio: "inherit" });

if (result.error) {
  console.error(`error: failed to execute domaingrep binary: ${result.error.message}`);
  process.exit(1);
}

process.exit(result.status === null ? 1 : result.status);
