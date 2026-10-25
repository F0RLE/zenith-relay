import { expect, test } from "bun:test";
import { startupAssets } from "../../scripts/check/frontend-bundle-assets.mjs";
import { staticModuleGraph } from "../build/startupChunks.ts";

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
