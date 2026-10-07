import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readdirSync } from "node:fs";
import { delimiter, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export function repoRoot() {
  return resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
}

function versionParts(value) {
  return value.split(".").map((part) => Number.parseInt(part, 10) || 0);
}

function compareVersions(left, right) {
  const a = versionParts(left);
  const b = versionParts(right);
  for (let index = 0; index < Math.max(a.length, b.length); index += 1) {
    const difference = (a[index] ?? 0) - (b[index] ?? 0);
    if (difference !== 0) return difference;
  }
  return 0;
}

function versionDirectories(root) {
  if (!root || !existsSync(root)) return [];
  return readdirSync(root, { withFileTypes: true })
    .filter((entry) => entry.isDirectory() && /^\d+(?:\.\d+)+$/.test(entry.name))
    .map((entry) => entry.name)
    .sort(compareVersions)
    .reverse();
}

function existingPathEntries(entries) {
  return entries.filter((entry) => entry && existsSync(entry));
}

function prependPath(env, entries) {
  const pathKey = process.platform === "win32" ? "Path" : "PATH";
  const current = env[pathKey] ?? env.PATH ?? "";
  const allEntries = [...existingPathEntries(entries), ...current.split(delimiter).filter(Boolean)];
  const seen = new Set();
  const unique = allEntries.filter((entry) => {
    const key = process.platform === "win32" ? entry.toLowerCase() : entry;
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
  env[pathKey] = unique.join(delimiter);
  env.PATH = env[pathKey];
}

function locateMsvcTools(root) {
  const versionsRoot = join(root, "VC", "Tools", "MSVC");
  for (const version of versionDirectories(versionsRoot)) {
    const install = join(versionsRoot, version);
    const bin = join(install, "bin", "Hostx64", "x64");
    if (
      existsSync(join(bin, "cl.exe")) &&
      existsSync(join(bin, "link.exe")) &&
      existsSync(join(install, "include")) &&
      existsSync(join(install, "lib", "x64"))
    ) {
      return { root, version, install, bin };
    }
  }
  return null;
}

function locateWindowsSdk(roots) {
  for (const root of roots.filter(Boolean)) {
    const includeRoot = join(root, "Include");
    const libRoot = join(root, "Lib");
    const binRoot = join(root, "bin");
    for (const version of versionDirectories(includeRoot)) {
      const include = join(includeRoot, version);
      const lib = join(libRoot, version);
      const bin = join(binRoot, version, "x64");
      if (
        existsSync(join(include, "ucrt")) &&
        existsSync(join(include, "um")) &&
        existsSync(join(include, "shared")) &&
        existsSync(join(lib, "ucrt", "x64")) &&
        existsSync(join(lib, "um", "x64")) &&
        existsSync(join(bin, "rc.exe"))
      ) {
        return { root, version, include, lib, bin };
      }
    }
  }
  return null;
}

function configureMsvcEnvironment(env, tools, sdk) {
  const includeEntries = [
    join(tools.install, "include"),
    join(tools.root, "VC", "Auxiliary", "VS", "include"),
    join(sdk.include, "ucrt"),
    join(sdk.include, "um"),
    join(sdk.include, "shared"),
    join(sdk.include, "winrt"),
    join(sdk.include, "cppwinrt"),
  ];
  const libEntries = [
    join(tools.install, "lib", "x64"),
    join(sdk.lib, "ucrt", "x64"),
    join(sdk.lib, "um", "x64"),
  ];

  env.VCINSTALLDIR = `${join(tools.root, "VC")}${process.platform === "win32" ? "\\" : ""}`;
  env.VCToolsInstallDir = `${tools.install}${process.platform === "win32" ? "\\" : ""}`;
  env.VCToolsVersion = tools.version;
  env.VSCMD_ARG_TGT_ARCH = "x64";
  env.WindowsSdkDir = `${sdk.root}${process.platform === "win32" ? "\\" : ""}`;
  env.WindowsSDKVersion = `${sdk.version}${process.platform === "win32" ? "\\" : ""}`;
  env.INCLUDE = existingPathEntries(includeEntries).join(";");
  env.LIB = existingPathEntries(libEntries).join(";");

  prependPath(env, [
    tools.bin,
    sdk.bin,
    join(tools.root, "MSBuild", "Current", "Bin", "amd64"),
  ]);
}

function configurePortableWindowsToolchain(env, developmentHome) {
  const configuredRoot = env.ZENITH_MSVC_ROOT;
  const portableRoot = configuredRoot || join(developmentHome, "visual-studio", "build-tools");
  const tools = locateMsvcTools(portableRoot);
  if (!tools) return false;

  const sdkRoots = [
    env.ZENITH_WINDOWS_SDK_ROOT,
    join(portableRoot, "Windows Kits", "10"),
    join(developmentHome, "windows-sdk"),
  ];
  const sdk = locateWindowsSdk(sdkRoots);
  if (!sdk) {
    throw new Error(
      `Portable MSVC was found at ${portableRoot}, but no Windows SDK was found. ` +
      "Install the SDK under Development/windows-sdk or set ZENITH_WINDOWS_SDK_ROOT.",
    );
  }

  configureMsvcEnvironment(env, tools, sdk);
  return true;
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

    const developmentHome = next.DEVELOPMENT_HOME
      || (next.USERPROFILE ? join(next.USERPROFILE, "Development") : "");
    const portableToolchainConfigured = configurePortableWindowsToolchain(next, developmentHome);

    const programFilesX86 = next["ProgramFiles(x86)"];
    const vswhere = programFilesX86
      ? join(programFilesX86, "Microsoft Visual Studio", "Installer", "vswhere.exe")
      : "";
    if (!portableToolchainConfigured && existsSync(vswhere)) {
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

    const visualStudioRoot = next.VSINSTALLDIR
      || (next.VCINSTALLDIR ? dirname(next.VCINSTALLDIR) : "")
      || (developmentHome ? join(developmentHome, "visual-studio", "build-tools") : "");
    const cmakeRoot = visualStudioRoot
      ? join(
          visualStudioRoot,
          "Common7",
          "IDE",
          "CommonExtensions",
          "Microsoft",
          "CMake",
        )
      : "";
    const cmakeBin = join(cmakeRoot, "CMake", "bin");
    const ninjaBin = join(cmakeRoot, "Ninja");
    const ninjaExecutable = join(ninjaBin, "ninja.exe");
    const toolPaths = [
      existsSync(join(cmakeBin, "cmake.exe")) ? cmakeBin : "",
      existsSync(join(ninjaBin, "ninja.exe")) ? ninjaBin : "",
      existsSync(join(visualStudioRoot, "MSBuild", "Current", "Bin", "amd64", "MSBuild.exe"))
        ? join(visualStudioRoot, "MSBuild", "Current", "Bin", "amd64")
        : "",
    ].filter(Boolean);

    if (developmentHome) {
      const nasmHome = join(developmentHome, "tools", "nasm-3.02");
      if (existsSync(join(nasmHome, "nasm.exe"))) {
        toolPaths.unshift(nasmHome);
      }
    }

    if (toolPaths.length > 0) {
      const existingPath = next[pathKey] ?? next.PATH ?? "";
      next[pathKey] = `${toolPaths.join(delimiter)}${delimiter}${existingPath}`;
      next.PATH = next[pathKey];
    }

    // The portable SDK is not registered with CMake. Ninja keeps native
    // builds on the configured MSVC/SDK environment instead of asking the
    // Visual Studio generator to discover a system SDK from the registry.
    if (portableToolchainConfigured && existsSync(ninjaExecutable)) {
      next.CMAKE_GENERATOR = "Ninja";
      next.CMAKE_MAKE_PROGRAM = ninjaExecutable;
    }

    // Keep MSBuild's temporary archive files inside Cargo's writable target tree.
    const buildTemp = join(repoRoot(), "target", "msbuild-temp");
    mkdirSync(buildTemp, { recursive: true });
    next.TEMP = buildTemp;
    next.TMP = buildTemp;
    next.AWS_LC_SYS_CMAKE_BUILDER ??= "1";
    next.CARGO_BUILD_JOBS ??= "1";
    next.CMAKE_BUILD_PARALLEL_LEVEL ??= "1";
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
