import { describe, expect, test } from "bun:test";
import { checkPlan } from "./ci-plan.mjs";

const sha = "a".repeat(40);
const repository = "synthetic/relay";
const env = {
  GITHUB_EVENT_NAME: "push", GITHUB_REF: "refs/heads/release/example",
  GITHUB_REPOSITORY: repository, GITHUB_SHA: sha,
};
const pull = {
  number: 7, state: "open", base: { ref: "main" },
  head: { ref: "release/example", sha, repo: { full_name: repository } },
};
const run = {
  event: "pull_request", head_sha: sha, status: "in_progress", conclusion: null,
  head_repository: { full_name: repository }, pull_requests: [{ number: 7, head: { sha } }],
};

function fixtureQuery(pulls = [[pull]], runs = [{ workflow_runs: [run] }]) {
  const calls = [];
  return {
    calls,
    query(endpoint, fields) {
      calls.push({ endpoint, fields });
      return endpoint.endsWith("/pulls") ? pulls : runs;
    },
  };
}

describe("CI duplicate-push planning", () => {
  test("only skips a release push with a matching open PR and an existing same-head PR run", () => {
    const { query, calls } = fixtureQuery();
    expect(checkPlan(env, query).runChecks).toBe(false);
    expect(calls).toEqual([
      { endpoint: `repos/${repository}/pulls`, fields: { state: "open", head: "synthetic:release/example", per_page: "100" } },
      { endpoint: `repos/${repository}/actions/workflows/build.yml/runs`, fields: { event: "pull_request", head_sha: sha, per_page: "100" } },
    ]);
  });

  test("PR, main, tags, manual runs and invalid context never query or skip", () => {
    for (const overrides of [
      { GITHUB_EVENT_NAME: "pull_request" }, { GITHUB_EVENT_NAME: "workflow_dispatch" },
      { GITHUB_REF: "refs/heads/main" }, { GITHUB_REF: "refs/tags/v1.2.3" },
      { GITHUB_REPOSITORY: "invalid" }, { GITHUB_SHA: "invalid" },
    ]) {
      const { query, calls } = fixtureQuery();
      expect(checkPlan({ ...env, ...overrides }, query).runChecks).toBe(true);
      expect(calls).toHaveLength(0);
    }
  });

  test("an absent, stale, foreign, closed or unsupported-base PR keeps branch coverage", () => {
    for (const pulls of [[], [{ ...pull, state: "closed" }], [{ ...pull, base: { ref: "other" } }],
      [{ ...pull, head: { ...pull.head, sha: "b".repeat(40) } }],
      [{ ...pull, head: { ...pull.head, ref: "other" } }],
      [{ ...pull, head: { ...pull.head, repo: { full_name: "foreign/relay" } } }]]) {
      const { query, calls } = fixtureQuery([pulls]);
      expect(checkPlan(env, query).runChecks).toBe(true);
      expect(calls).toHaveLength(1);
    }
  });

  test("the PR-run race, cancellation, missing associations and foreign runs cannot skip checks", () => {
    for (const runs of [[], [{ ...run, head_sha: "b".repeat(40) }],
      [{ ...run, event: "push" }], [{ ...run, conclusion: "cancelled" }],
      [{ ...run, conclusion: "skipped" }], [{ ...run, conclusion: "action_required" }],
      [{ ...run, status: "unknown" }], [{ ...run, pull_requests: [] }],
      [{ ...run, pull_requests: [{ number: 8, head: { sha } }] }],
      [{ ...run, pull_requests: [{ number: 7, head: { sha: "b".repeat(40) } }] }],
      [{ ...run, head_repository: { full_name: "foreign/relay" } }]]) {
      const { query } = fixtureQuery([[pull]], [{ workflow_runs: runs }]);
      expect(checkPlan(env, query).runChecks).toBe(true);
    }
  });

  test("pagination and PRs into release branches retain exact-head matching", () => {
    const { query } = fixtureQuery([[], [{ ...pull, base: { ref: "release/next" } }]], [{ workflow_runs: [] }, { workflow_runs: [run] }]);
    expect(checkPlan(env, query).runChecks).toBe(false);
  });

  test("a failing PR remains the authoritative failing check, not a duplicate push", () => {
    const { query } = fixtureQuery([[pull]], [{ workflow_runs: [{ ...run, status: "completed", conclusion: "failure" }] }]);
    expect(checkPlan(env, query).runChecks).toBe(false);
  });

  test("API failures and malformed responses fall back to complete checks", () => {
    for (const query of [() => { throw new Error("synthetic unavailable API"); }, () => null,
      (endpoint) => endpoint.endsWith("/pulls") ? [[pull]] : null]) {
      expect(checkPlan(env, query).runChecks).toBe(true);
    }
  });
});
