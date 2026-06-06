import { existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export function repoRoot() {
  return resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
}

export function withZenithRustEnv(env = process.env) {
  const next = { ...env };
  const nodeBin = dirname(process.execPath);
  const pathKey = process.platform === "win32" ? "Path" : "PATH";
  const existingPath = next[pathKey] ?? next.PATH ?? "";
  next[pathKey] = `${nodeBin};${existingPath}`;
  next.PATH = next[pathKey];

  if (process.platform === "win32") {
    const localAppData = next.LOCALAPPDATA;
    if (localAppData) {
      const rustRoot = join(localAppData, "Zenith", "tools", "rust");
      const cargoHome = join(rustRoot, "cargo-home");
      const rustupHome = join(rustRoot, "rustup-home");
      const cargoBin = join(cargoHome, "bin");

      if (existsSync(join(cargoBin, "cargo.exe"))) {
        next.CARGO_HOME = cargoHome;
        next.RUSTUP_HOME = rustupHome;
        next[pathKey] = `${cargoBin};${next[pathKey]}`;
        next.PATH = next[pathKey];
      }
    }
  }

  return next;
}

export function tauriInvocation(args) {
  const localCli = join(repoRoot(), "src", "node_modules", "@tauri-apps", "cli", "tauri.js");

  if (existsSync(localCli)) {
    return {
      command: process.execPath,
      args: [localCli, ...args],
      shell: false,
    };
  }

  return {
    command: process.platform === "win32" ? "tauri.cmd" : "tauri",
    args,
    shell: process.platform === "win32",
  };
}
