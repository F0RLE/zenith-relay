import { expect, test } from "bun:test";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { startupAssets } from "../../scripts/check/frontend-bundle-assets.mjs";
import { staticModuleGraph, startupChunks } from "../build/startupChunks.ts";
import { minifyStartup } from "../build/minifyStartup.mjs";

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

test("only dependencies already shared by Pool and Connections join their companion chunk", () => {
  const root = "/app";
  const main = `${root}/src/main.tsx`;
  const bootstrap = `${root}/src/bootstrap.ts`;
  const pool = `${root}/src/features/relay/pages/pool/PoolPage.tsx`;
  const connections = `${root}/src/features/relay/pages/connections/ConnectionsPage.tsx`;
  const modules = {
    [main]: { importedIds: ["react"], dynamicImports: [pool, connections] },
    [bootstrap]: { importedIds: ["native"] },
    [pool]: { importedIds: ["react", "details", "pool-only"] },
    [connections]: { importedIds: ["react", "details", "connections-only"], dynamicImports: ["import-dialog"] },
    details: { importedIds: ["formatter"] }, formatter: { importedIds: [] },
    react: { importedIds: [] }, native: { importedIds: [] },
    "pool-only": { importedIds: [] }, "connections-only": { importedIds: [] }, "import-dialog": { importedIds: [] },
  };
  const plugin = startupChunks();
  plugin.configResolved({ root });
  plugin.buildStart();
  plugin.buildEnd.call({ getModuleInfo: id => modules[id], error: message => { throw Error(message); } }, null);
  const groups = plugin.config().build.rolldownOptions.output.codeSplitting.groups;
  const companion = groups.find(group => group.name === "connection-details");
  for (const id of ["details", "formatter"]) expect(companion.test(id)).toBe(true);
  for (const id of [main, bootstrap, pool, connections, "react", "native", "pool-only", "connections-only", "import-dialog"]) {
    expect(companion.test(id)).toBe(false);
  }
  expect(groups.find(group => group.name === "workspace").test("details")).toBe(false);
  plugin.buildStart();
  expect(companion.test("details")).toBe(false);
});

test("production minifies the independent startup watchdog without bundling dependencies", async () => {
  const publicDir = fileURLToPath(new URL("../public", import.meta.url));
  const original = await readFile(new URL("../public/startup.js", import.meta.url), "utf8");
  const plugin = minifyStartup();
  const emitted = [];
  const watched = [];
  expect(plugin.apply).toBe("build");
  plugin.configResolved({ publicDir });
  await plugin.generateBundle.call({
    addWatchFile: path => watched.push(path), emitFile: asset => emitted.push(asset),
    error: message => { throw Error(message); },
  });
  expect(watched).toHaveLength(1);
  expect(emitted).toHaveLength(1);
  expect(emitted[0].fileName).toBe("startup.js");
  expect(emitted[0].type).toBe("asset");
  expect(emitted[0].source.length).toBeLessThan(original.length);
  for (const event of ["zenith-startup-ready", "zenith-startup-phase", "zenith:html-start", "splash-reload"]) {
    expect(emitted[0].source).toContain(event);
  }
  expect(emitted[0].source).not.toMatch(/\b(?:import|require)\s*\(/);
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
