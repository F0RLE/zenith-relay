import type { Root, RootContent } from "mdast";

type SearchOptions = { query: string; empty: string; results: (count: number) => string };

function nodeText(node: { value?: string; children?: unknown[] }): string {
  return node.value ?? (node.children ?? []).map((child) => nodeText(child as typeof node)).join(" ");
}

export function remarkErrorReference({ query, empty, results }: SearchOptions) {
  return (tree: Root) => {
    const terms = query.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean);
    const output: RootContent[] = [];
    let matches = 0;
    let reachedGroups = false;
    for (let index = 0; index < tree.children.length; index++) {
      const heading = tree.children[index];
      const table = tree.children[index + 1];
      if (!heading) continue;
      if (heading.type !== "heading" || heading.depth !== 3 || table?.type !== "table" || !table.children[0]) {
        if (!terms.length || reachedGroups) output.push(heading);
        continue;
      }
      index++;
      reachedGroups = true;
      const rows = table.children.slice(1).filter((row) => {
        const text = `${nodeText(heading)} ${nodeText(row)}`.toLocaleLowerCase();
        return terms.every((term) => text.includes(term));
      });
      matches += rows.length;
      if (!rows.length) continue;
      output.push({
        type: "blockquote",
        data: { hName: "details", hProperties: { className: ["help-error-group"], open: terms.length > 0 } },
        children: [
          { type: "paragraph", data: { hName: "summary" }, children: [
            ...heading.children,
            { type: "text", value: String(rows.length), data: { hName: "span", hProperties: { className: ["help-error-count"] } } },
          ] },
          { ...table, children: [table.children[0], ...rows] },
        ],
      });
    }
    if (terms.length) {
      output.unshift({ type: "paragraph", data: { hName: "div", hProperties: { role: "status", className: ["help-error-results"] } },
        children: [{ type: "text", value: matches ? results(matches) : empty }] });
    }
    tree.children = output;
  };
}
