import { spawnSync } from "node:child_process";

function githubPages(endpoint, fields) {
  const result = spawnSync("gh", [
    "api", "--method", "GET", "--paginate", "--slurp", endpoint,
    ...Object.entries(fields).flatMap(([key, value]) => ["-f", `${key}=${value}`]),
  ], { encoding: "utf8", shell: false, windowsHide: true, timeout: 30_000, maxBuffer: 2 * 1024 * 1024 });
  if (result.error || result.status !== 0) throw new Error("GitHub check-plan lookup failed.");
  const pages = JSON.parse(result.stdout);
  if (!Array.isArray(pages)) throw new Error("Invalid GitHub pagination response.");
  return pages;
}

export function checkPlan(env, query = githubPages) {
  const run = { runChecks: true, reason: "Run the complete check suite." };
  if (env.GITHUB_EVENT_NAME !== "push" || !env.GITHUB_REF?.startsWith("refs/heads/release/")) return run;
  const repository = env.GITHUB_REPOSITORY;
  const sha = env.GITHUB_SHA;
  if (!/^[\w.-]+\/[\w.-]+$/.test(repository ?? "") || !/^[a-f0-9]{40}$/.test(sha ?? "")) return run;
  const branch = env.GITHUB_REF.slice("refs/heads/".length);
  const sameRepository = (value) => value?.toLowerCase() === repository.toLowerCase();
  try {
    const pulls = query(`repos/${repository}/pulls`, {
      state: "open", head: `${repository.split("/")[0]}:${branch}`, per_page: "100",
    }).flat();
    const numbers = new Set(pulls.filter((pull) => pull.state === "open"
      && sameRepository(pull.head?.repo?.full_name) && pull.head?.ref === branch && pull.head?.sha === sha
      && (pull.base?.ref === "main" || pull.base?.ref?.startsWith("release/")))
      .map((pull) => pull.number).filter(Number.isSafeInteger));
    if (numbers.size === 0) return run;
    const pages = query(`repos/${repository}/actions/workflows/build.yml/runs`, {
      event: "pull_request", head_sha: sha, per_page: "100",
    });
    const covered = pages.flatMap((page) => page.workflow_runs ?? []).some((candidate) =>
      candidate.event === "pull_request" && candidate.head_sha === sha
      && sameRepository(candidate.head_repository?.full_name)
      && ["queued", "pending", "waiting", "requested", "in_progress", "completed"].includes(candidate.status)
      && !["cancelled", "skipped", "action_required"].includes(candidate.conclusion)
      && candidate.pull_requests?.some((pull) => numbers.has(pull.number) && pull.head?.sha === sha));
    return covered ? { runChecks: false, reason: "This release-branch commit already has its own PR Build run." } : run;
  } catch {
    // Missing permissions, races and API outages must add work, never remove coverage.
    return { runChecks: true, reason: "PR coverage could not be confirmed; run the complete check suite." };
  }
}

if (import.meta.main) {
  const plan = checkPlan(process.env);
  console.error(`[ci-plan] ${plan.reason}`);
  console.log(`run-checks=${plan.runChecks}`);
}
