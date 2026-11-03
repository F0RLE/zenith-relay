import { describe, expect, test } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";
import { LocaleProvider } from "../../src/i18n/LocaleProvider";
import { OwnerProxies } from "../../src/features/owner/OwnerProxies";
import { assignProxyAccounts, parseProxyImport, safeProxyEndpoint } from "../../src/features/owner/proxyPresentation";
import type { OwnerProxy } from "../../src/features/owner/proxyPresentation";

const proxy = (id: string, assignedAccountIds: string[] = []): OwnerProxy => ({ id, endpoint: `http://${id}.example:8080`, assignedAccountIds, createdAtMs: 0 });

describe("admin proxy preview and safe summaries", () => {
  test("launcher import formats produce safe endpoints, not stored credentials", () => {
    const result = parseProxyImport("host.example:8080:demo-user:demo-pass\ndemo-user:demo-pass@second.example:3128\nhttps://demo-user:demo-pass@secure.example:443\nsocks5://demo-user:demo-pass@[2001:db8::1]:1080\nsocks5h://remote.example:1080");
    expect(result.invalidLines).toEqual([]);
    expect(result.entries).toHaveLength(5);
    expect(result.entries.map(item => item.hasAuthentication)).toEqual([true, true, true, true, false]);
    expect(result.entries[0]!.endpoint).toBe("http://host.example:8080");
    expect(JSON.stringify(result)).not.toContain("demo-user");
    expect(JSON.stringify(result)).not.toContain("demo-pass");
  });

  test("bad rows are identified by number without echoing private input", () => {
    const result = parseProxyImport("\nftp://private-user:private-pass@host.example:21\nhttp://host.example:0\nhttp://host.example:65536\nhttp://host.example:8080/path?token=private-token\nnot-a-proxy");
    expect(result.invalidLines).toEqual([2, 3, 4, 5, 6]);
    expect(result.entries).toEqual([]);
    expect(JSON.stringify(result)).not.toMatch(/private-user|private-pass|private-token/);
    expect(parseProxyImport(Array(201).fill("host.example:80").join("\n")).tooMany).toBe(true);
  });

  test("duplicates are bounded to known identities, not just shared authenticated hosts", () => {
    const result = parseProxyImport("http://host.example:8080\nhost.example:8080\nhttp://first:demo-pass@host.example:8080\nhttp://second:demo-pass@host.example:8080");
    expect(result.duplicates).toBe(1);
    expect(result.entries).toHaveLength(3);
    expect(parseProxyImport("http://saved.example:8080", [proxy("saved")]).duplicates).toBe(1);
    expect(safeProxyEndpoint("http://demo-user:demo-pass@host.example:8080")).toBe("http://host.example:8080");
    expect(safeProxyEndpoint("file:///private")).toBeNull();
  });

  test("account reassignment is exclusive, preserves hidden bindings and does not mutate input", () => {
    const pool = [proxy("first", ["account-a", "hidden-account"]), proxy("second", ["account-b"])];
    const original = JSON.stringify(pool);
    const result = assignProxyAccounts(pool, "first", ["account-b", "account-b", "not-allowed"], ["account-a", "account-b"]);
    expect(result[0]!.assignedAccountIds).toEqual(["hidden-account", "account-b"]);
    expect(result[1]!.assignedAccountIds).toEqual([]);
    expect(JSON.stringify(pool)).toBe(original);
    expect(assignProxyAccounts(pool, "missing", ["account-b"], ["account-b"])).toEqual(pool);
  });

  test("an unmeasured proxy is not shown as healthy, located or measured", () => {
    const html = renderToStaticMarkup(<LocaleProvider><OwnerProxies entries={[{ ...proxy("first"), endpoint: "http://demo-user:demo-pass@proxy.example:8080" }]} accounts={[]} onChange={() => {}} active importOpen={false} onImportClose={() => {}} onImport={() => {}} /></LocaleProvider>);
    expect(html).toContain("Not checked");
    expect(html).not.toContain("Connection verified");
    expect(html).not.toContain("demo-user");
    expect(html).not.toContain("demo-pass");
    expect(html).not.toContain("0 ms");
    expect(html).toContain('data-state="unknown"');
  });
});
