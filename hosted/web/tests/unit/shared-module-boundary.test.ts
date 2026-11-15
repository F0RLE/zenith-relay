import { expect, test } from "bun:test";
import { relative, resolve, sep } from "node:path";

test("Hosted compiler inputs stay independent of Desktop dependencies", () => {
  const web = resolve(import.meta.dir, "../..");
  const desktop = resolve(import.meta.dir, "../../../src");
  const result = Bun.spawnSync([
    process.execPath, resolve(web, "node_modules/typescript/bin/tsc"),
    "--project", resolve(web, "tsconfig.json"), "--listFilesOnly",
  ], { cwd: web });
  expect(result.exitCode).toBe(0);
  const files = result.stdout.toString().trim().split(/\r?\n/);
  expect(files.length).toBeGreaterThan(0);
  const borrowed = files
    .map(file => relative(desktop, file))
    .filter(path => path.startsWith(`node_modules${sep}`));
  expect(borrowed).toEqual([]);
});
