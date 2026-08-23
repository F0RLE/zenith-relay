import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { delimiter, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export function repoRoot() {
  return resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
}

export function withZenithRustEnv(env = process.env) {
  const next = { ...env };
  const nodeBin = dirname(process.execPath);
  const pathKey = process.platform === "win32" ? "Path" : "PATH";
  const existingPath = next[pathKey] ?? next.PATH ?? "";
  next[pathKey] = `${nodeBin}${delimiter}${existingPath}`;
  next.PATH = next[pathKey];

  if (process.platform === "win32") {
    const probe = spawnSync("rustc", ["--print", "sysroot"], {
      env: next,
      encoding: "utf8",
      shell: true,
      windowsHide: true,
    });
    const sysroot = probe.status === 0 ? probe.stdout.trim() : "";
    const toolchainBin = join(sysroot, "bin");
    if (sysroot && existsSync(join(toolchainBin, "cargo.exe"))) {
      const rustupHome = dirname(dirname(sysroot));
      const cargoHome = join(dirname(rustupHome), "cargo-home");
      if (existsSync(join(cargoHome, "bin", "cargo.exe"))) {
        next.CARGO_HOME = cargoHome;
        next.RUSTUP_HOME = rustupHome;
      }
      next[pathKey] = `${toolchainBin}${delimiter}${next[pathKey]}`;
      next.PATH = next[pathKey];
    }

    const programFilesX86 = next["ProgramFiles(x86)"];
    const vswhere = programFilesX86
      ? join(programFilesX86, "Microsoft Visual Studio", "Installer", "vswhere.exe")
      : "";
    if (existsSync(vswhere)) {
      const located = spawnSync(
        vswhere,
        [
          "-latest",
          "-products",
          "*",
          "-requires",
          "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
          "-property",
          "installationPath",
        ],
        { encoding: "utf8", windowsHide: true },
      );
      const install = located.status === 0 ? located.stdout.trim() : "";
      const vcvars = join(install, "VC", "Auxiliary", "Build", "vcvars64.bat");
      if (install && existsSync(vcvars)) {
        const initialized = spawnSync(`chcp 65001 >nul && call "${vcvars}" >nul && set`, {
          env: next,
          encoding: "utf8",
          shell: true,
          windowsHide: true,
        });
        if (initialized.status === 0) {
          for (const line of initialized.stdout.split(/\r?\n/)) {
            const separator = line.indexOf("=");
            if (separator > 0) next[line.slice(0, separator)] = line.slice(separator + 1);
          }
          next.PATH = next[pathKey];
        }
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
