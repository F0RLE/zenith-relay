import { expect, test } from "bun:test";
import { startupAssets } from "../../scripts/check/frontend-bundle-assets.mjs";
import { staticModuleGraph, startupChunks } from "../build/startupChunks.ts";

test("shared value helpers join the workspace without pulling lazy pages or the native loader", () => {
  const root = "/app";
  const main = `${root}/src/main.tsx`;
  const bootstrap = `${root}/src/bootstrap.ts`;
  const value = `${root}/src/features/relay/accountEconomics.ts`;
  const clock = `${root}/src/features/relay/hooks/useRelativeTimeClock.ts`;
  const modules = {
    [main]: { importedIds: ["react"], dynamicImports: ["pool"] },
    [bootstrap]: { importedIds: ["native"] },
    [value]: { importedIds: ["formatting"] },
    [clock]: { importedIds: ["react"] },
    formatting: { importedIds: [] }, react: { importedIds: [] },
    native: { importedIds: [] }, pool: { importedIds: [value] },
  };
  const plugin = startupChunks();
  plugin.configResolved({ root });
  plugin.buildStart();
  plugin.buildEnd.call({ getModuleInfo: id => modules[id], error: message => { throw Error(message); } }, null);
  const groups = plugin.config().build.rolldownOptions.output.codeSplitting.groups;
  const workspace = groups.find(group => group.name === "workspace");
  for (const id of [value, clock, "formatting", "react"]) expect(workspace.test(id)).toBe(true);
  for (const id of ["native", "pool", main]) expect(workspace.test(id)).toBe(false);
});

test("startup grouping follows static dependencies through cycles and ignores external modules", () => {
  const modules = {
    main: { importedIds: ["react", "shared", "external"], dynamicImports: ["settings"] },
    react: { importedIds: [] },
    shared: { importedIds: ["main"] },
    external: { importedIds: [], isExternal: true },
    settings: { importedIds: ["markdown"] },
  };
  expect([...staticModuleGraph(["main"], id => modules[id])].sort()).toEqual(["main", "react", "shared"]);
});

test("the loader graph stays separate from the workspace and its lazy pages", () => {
  const modules = {
    bootstrap: { importedIds: ["native"], dynamicImports: ["main"] },
    native: { importedIds: [] },
    main: { importedIds: ["native", "react"], dynamicImports: ["usage"] },
    react: { importedIds: [] },
    usage: { importedIds: [] },
  };
  const lookup = id => modules[id];
  const loader = staticModuleGraph(["bootstrap"], lookup);
  const workspace = [...staticModuleGraph(["main"], lookup)].filter(id => id !== "main" && !loader.has(id));
  expect([...loader].sort()).toEqual(["bootstrap", "native"]);
  expect(workspace).toEqual(["react"]);
});

test("startup budgets include the eager UI, its CSS and shared imports, but not lazy pages", () => {
  expect(startupAssets({
    "index.html": { file: "loader.js", dynamicImports: ["src/main.tsx"] },
    "src/main.tsx": { file: "main.js", css: ["main.css"], imports: ["shared", "index.html"], dynamicImports: ["settings"] },
    shared: { file: "shared.js", css: ["main.css"], imports: ["index.html"] },
    settings: { file: "settings.js", css: ["settings.css"] },
  })).toEqual(["loader.js", "main.js", "main.css", "shared.js"]);
});

test("a missing UI entry or static dependency fails the budget check", () => {
  expect(() => startupAssets({ "index.html": { file: "loader.js" } })).toThrow("src/main.tsx");
  expect(() => startupAssets({
    "index.html": { file: "loader.js" },
    "src/main.tsx": { file: "main.js", imports: ["missing"] },
  })).toThrow("missing");
});
