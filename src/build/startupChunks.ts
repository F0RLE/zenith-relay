import type { Plugin } from "vite";

type ModuleDependencies = { importedIds: readonly string[]; isExternal?: boolean };

export function staticModuleGraph(
  roots: readonly string[],
  lookup: (id: string) => ModuleDependencies | null | undefined,
) {
  const modules = new Set<string>();
  const pending = [...roots];
  while (pending.length) {
    const id = pending.pop()!;
    if (modules.has(id)) continue;
    const info = lookup(id);
    if (!info || info.isExternal) continue;
    modules.add(id);
    pending.push(...info.importedIds);
  }
  return modules;
}

export function startupChunks(): Plugin {
  const bootstrap = new Set<string>();
  const workspace = new Set<string>();
  let bootstrapEntry = "";
  let workspaceEntry = "";
  let sharedEntries: string[] = [];
  return {
    name: "relay-startup-chunks",
    apply: "build",
    config: () => ({ build: { rolldownOptions: { output: { codeSplitting: { groups: [
      { name: "bootstrap", test: (id: string) => bootstrap.has(id), priority: 30 },
      { name: "workspace", test: (id: string) => workspace.has(id), priority: 20 },
    ] } } } } }),
    configResolved(config) {
      const root = config.root.replaceAll("\\", "/");
      bootstrapEntry = `${root}/src/bootstrap.ts`;
      workspaceEntry = `${root}/src/main.tsx`;
      // These small helpers are shared by Connections, Pool and Usage. Keep
      // their common code in one dictionary rather than extra shared chunks.
      sharedEntries = [
        `${root}/src/features/relay/accountEconomics.ts`,
        `${root}/src/features/relay/hooks/useRelativeTimeClock.ts`,
        `${root}/src/features/relay/hooks/useProxyPool.ts`,
        `${root}/src/features/relay/state/selectionModel.ts`,
        `${root}/src/features/relay/pages/usage/usageFormatting.ts`,
        `${root}/src/features/relay/sourceUrl.ts`,
      ];
    },
    buildStart() {
      bootstrap.clear();
      workspace.clear();
    },
    buildEnd(error) {
      if (error) return;
      for (const root of [bootstrapEntry, workspaceEntry]) {
        if (!this.getModuleInfo(root)) this.error(`Missing startup module: ${root}`);
      }
      const lookup = (id: string) => this.getModuleInfo(id);
      for (const id of staticModuleGraph([bootstrapEntry], lookup)) bootstrap.add(id);
      for (const id of staticModuleGraph([workspaceEntry, ...sharedEntries], lookup)) {
        // Keep the native loader independent and preserve main's dynamic-entry facade.
        if (id !== workspaceEntry && !bootstrap.has(id)) workspace.add(id);
      }
    },
  };
}
