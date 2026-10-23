/** Count the workspace too: the loader imports it immediately, not on user intent. */
export function startupAssets(manifest) {
  const visited = new Set();
  const assets = new Set();
  function visit(key) {
    if (visited.has(key)) return;
    const chunk = manifest[key];
    if (!chunk) throw new Error(`Startup chunk is missing from the build manifest: ${key}`);
    visited.add(key);
    assets.add(chunk.file);
    for (const css of chunk.css ?? []) assets.add(css);
    for (const dependency of chunk.imports ?? []) visit(dependency);
  }
  visit("index.html");
  visit("src/main.tsx");
  return [...assets];
}
