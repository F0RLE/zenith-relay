import { expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const script = fileURLToPath(new URL("../release/publish-updater-manifest.ps1", import.meta.url));
const shell = process.platform === "win32" ? "powershell.exe" : "pwsh";
const suffixes = {
  "macos-arm64": [".app.tar.gz"],
  "macos-intel": [".app.tar.gz"],
  "linux-arm64": [".AppImage", ".deb", ".rpm"],
  "linux-x64": [".AppImage", ".deb", ".rpm"],
  "windows-arm64": ["-setup.exe", ".exe"],
  "windows-x64": ["-setup.exe", ".exe"],
};

function withArtifacts(run) {
  const root = mkdtempSync(join(tmpdir(), "relay-updater-manifest-"));
  try {
    const artifacts = join(root, "artifacts");
    const output = join(root, "output");
    for (const [platform, extensions] of Object.entries(suffixes)) {
      const folder = join(artifacts, `zenith-relay-${platform}`);
      mkdirSync(folder, { recursive: true });
      for (const extension of extensions) {
        const asset = join(folder, `zenith-relay-${platform}${extension}`);
        writeFileSync(asset, "synthetic release artifact");
        writeFileSync(`${asset}.sig`, "synthetic-signature");
      }
    }
    const invoke = (tag = "v1.1.5") => {
      const result = spawnSync(shell, [
        "-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
        "-File", script, "-TagName", tag, "-Repository", "synthetic/relay",
        "-ArtifactsDir", artifacts, "-OutputDir", output, "-Notes", "Synthetic release notes",
      ], { encoding: "utf8", timeout: 30_000, windowsHide: true });
      if (result.error) throw result.error;
      return { ...result, text: result.stdout + result.stderr };
    };
    return run({ root, artifacts, output, invoke });
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

test("updater manifest preserves every platform, signature and release note", () => {
  withArtifacts(({ output, invoke }) => {
    const result = invoke();
    expect(result.status, result.text).toBe(0);
    const manifest = JSON.parse(readFileSync(join(output, "latest.json"), "utf8").replace(/^\uFEFF/, ""));
    expect(manifest.version).toBe("1.1.5");
    expect(manifest.notes).toBe("Synthetic release notes");
    expect(Object.keys(manifest.platforms)).toHaveLength(18);
    for (const platform of Object.values(manifest.platforms)) {
      expect(platform.signature).toBe("synthetic-signature");
      expect(platform.url).toStartWith("https://github.com/synthetic/relay/releases/download/v1.1.5/");
      expect(existsSync(join(output, platform.url.split("/").at(-1)))).toBe(true);
    }
  });
}, 30_000);

test("updater preparation preserves an existing nonempty output directory", () => {
  withArtifacts(({ output, invoke }) => {
    mkdirSync(output);
    const saved = join(output, "keep.txt");
    writeFileSync(saved, "existing data");
    const result = invoke();
    expect(result.status).not.toBe(0);
    expect(result.text).toContain("Updater output must be empty");
    expect(readFileSync(saved, "utf8")).toBe("existing data");
    expect(existsSync(join(output, "latest.json"))).toBe(false);
  });
}, 30_000);

test.each(["missing", "empty", "whitespace signature", "duplicate"])("updater rejects %s artifacts before writing a manifest", (kind) => {
  withArtifacts(({ artifacts, output, invoke }) => {
    const folder = join(artifacts, "zenith-relay-macos-arm64");
    const name = "zenith-relay-macos-arm64.app.tar.gz";
    const asset = join(folder, name);
    if (kind === "missing") rmSync(asset);
    if (kind === "empty") writeFileSync(asset, "");
    if (kind === "whitespace signature") writeFileSync(`${asset}.sig`, " \r\n");
    if (kind === "duplicate") {
      mkdirSync(join(folder, "duplicate"));
      writeFileSync(join(folder, "duplicate", name), "ambiguous artifact");
    }
    expect(invoke().status).not.toBe(0);
    expect(existsSync(join(output, "latest.json"))).toBe(false);
  });
}, 30_000);

test("stable updater rejects a prerelease before creating output", () => {
  withArtifacts(({ output, invoke }) => {
    expect(invoke("v1.1.5-rc.1").status).not.toBe(0);
    expect(existsSync(output)).toBe(false);
  });
}, 30_000);
