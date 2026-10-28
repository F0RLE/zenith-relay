import { expect, test } from "bun:test";
import { tauriBuildArgs } from "./tauri-build-args.mjs";

test("an unsigned build passes configuration to Tauri and locked dependencies to Cargo", () => {
  const args = tauriBuildArgs(["--no-bundle"]);
  const separator = args.indexOf("--");
  expect(args.slice(0, 4)).toEqual(["build", "--config", "src-tauri/tauri.conf.json", "--no-bundle"]);
  expect(JSON.parse(args[separator - 1])).toEqual({ bundle: { createUpdaterArtifacts: false } });
  expect(args.slice(separator + 1)).toEqual(["--locked"]);
});

test("custom Cargo arguments do not swallow the unsigned Tauri configuration", () => {
  const args = tauriBuildArgs(["--verbose", "--", "--profile", "dev"]);
  const separator = args.indexOf("--");
  expect(args.filter(arg => arg === "--")).toHaveLength(1);
  expect(args.slice(separator + 1)).toEqual(["--locked", "--profile", "dev"]);
  expect(args.slice(0, separator)).toContain("--config");
  expect(args.slice(separator + 1)).not.toContain("--config");
});

test("signed builds retain updater artifacts and custom Tauri options", () => {
  expect(tauriBuildArgs(["--config", "extra.json"], { signingKeyPresent: true })).toEqual([
    "build", "--config", "src-tauri/tauri.conf.json", "--config", "extra.json", "--", "--locked",
  ]);
});

test.each(["--locked", "--frozen"])("an explicit %s is not duplicated", (flag) => {
  const args = tauriBuildArgs(["--no-bundle", "--", flag]);
  expect(args.slice(args.indexOf("--") + 1)).toEqual([flag]);
});
