import { expect, test } from "bun:test";
import { startupAssets } from "../../scripts/check/frontend-bundle-assets.mjs";

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
