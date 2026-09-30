import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { validatePullRequest } from "./pr-metadata.mjs";

const template = readFileSync(new URL("../PULL_REQUEST_TEMPLATE.md", import.meta.url), "utf8");
const completed = template
  .replaceAll("- [ ] I ", "- [x] I ")
  .replace("- [ ] No release note", "- [x] No release note");
const confirmations = completed.split(/\r?\n/).filter((line) => line.startsWith("- [x] I "));
const human = { type: "User", login: "example-contributor" };
const check = (body = completed) => validatePullRequest({ user: human, body }).errors;

describe("contributor consent", () => {
  test("the repository template passes after personal confirmation and a release decision", () => {
    expect(confirmations).toHaveLength(1);
    expect(check()).toEqual([]);
  });

  test("an untouched template does not imply consent", () => {
    expect(check(template).filter((error) => error.includes("Contributor Agreement"))).toHaveLength(1);
  });

  test("clearing the single checkbox withdraws consent", () => {
    expect(check(completed.replace(confirmations[0], confirmations[0].replace("[x]", "[ ]"))))
      .toContain("read the linked Contributor Agreement and check its single checkbox yourself, keeping the original wording");
  });

  test.each([
    "Contributor Agreement v0.9",
    "Contributor Agreement v2.0",
    "Contributor Agreement v1.0 except its copyright assignment",
  ])("a changed agreement or version does not pass: %s", (agreement) => {
    expect(check(completed.replace("Contributor Agreement v1.0", agreement)).length).toBeGreaterThan(0);
  });

  test.each([
    "including its copyright assignment",
    "If this PR is accepted, I assign to F0RLE the copyright I own in my original changes",
    "and confirm that I have permission to submit them",
  ])("the single checkbox cannot omit its substantive terms: %s", (clause) => {
    expect(check(completed.replace(clause, "")).length).toBeGreaterThan(0);
  });

  test("replacing assignment with an AGPL-only statement fails", () => {
    expect(check(completed.replace(confirmations[0], "- [x] I only license this contribution under AGPL-3.0-only.")).length)
      .toBeGreaterThan(0);
  });

  test("the removed target branch checkbox is not required", () => {
    expect(template).not.toContain("This PR targets");
  });

  test("the linked agreement cannot be replaced with an unrelated document", () => {
    expect(check(completed.replace("/CONTRIBUTOR_LICENSE_AGREEMENT.md)", "/LICENSE)")).length)
      .toBeGreaterThan(0);
  });

  test("the old three-checkbox template is not mistaken for the new confirmation", () => {
    const oldConfirmations = [
      "- [x] I have read and agree to Contributor Agreement v1.0 (CONTRIBUTOR_LICENSE_AGREEMENT.md), including its copyright assignment.",
      "- [x] I assign the copyright I own in this PR's original contributions to F0RLE upon acceptance under that agreement.",
      "- [x] I have authority to make this assignment and have disclosed all co-authored, employer-owned, pre-existing, and third-party material.",
    ].join("\n");
    expect(check(completed.replace(confirmations[0], oldConfirmations)).length).toBeGreaterThan(0);
  });

  test.each([
    ["HTML comment", (line) => `<!--\n${line}\n-->`],
    ["fenced example", (line) => `\`\`\`markdown\n${line}\n\`\`\``],
    ["tilde fence", (line) => `~~~~\n${line}\n~~~~~`],
    ["indented code", (line) => `    ${line}`],
    ["quoted text", (line) => `> ${line}`],
  ])("a confirmation inside %s is not consent", (_name, hide) => {
    expect(check(completed.replace(confirmations[0], hide(confirmations[0]))).length).toBeGreaterThan(0);
  });

  test("an unterminated fence or HTML comment cannot reveal hidden consent", () => {
    expect(check(`\`\`\`\n${completed}`).length).toBeGreaterThan(0);
    expect(check(`<!--\n${completed}`).length).toBeGreaterThan(0);
  });

  test("confirmations from another section are rejected", () => {
    const body = completed.replace(confirmations.join("\n"), "")
      .replace("## Validation", `## Validation\n${confirmations.join("\n")}`);
    expect(check(body).filter((error) => error.includes("Contributor Agreement"))).toHaveLength(1);
  });

  test("duplicated or contradictory confirmations fail", () => {
    for (const duplicate of [confirmations[0], confirmations[0].replace("[x]", "[ ]")]) {
      expect(check(completed.replace(confirmations[0], `${confirmations[0]}\n${duplicate}`)).length)
        .toBeGreaterThan(0);
    }
  });

  test("duplicate agreement sections fail instead of choosing the favorable one", () => {
    expect(check(`${completed}\n## Contributor Agreement\n${confirmations.join("\n")}`))
      .toContain("include exactly one ## Contributor Agreement section");
  });

  test("ordinary GitHub checkbox capitalization and Windows line endings work", () => {
    expect(check(completed.replaceAll("[x]", "[X]").replace(/\r?\n/g, "\r\n"))).toEqual([]);
  });

  test("an absent body fails without throwing", () => {
    expect(validatePullRequest({ user: human, body: null }).errors.length).toBeGreaterThan(0);
  });
});

describe("release metadata", () => {
  test("both release choices cannot be checked together", () => {
    expect(check(completed.replace("- [ ] Release-worthy", "- [x] Release-worthy")))
      .toContain("select exactly one Release Notes checkbox");
  });

  test("release-worthy changes need actual note text", () => {
    const release = completed
      .replace("- [x] No release note", "- [ ] No release note")
      .replace("- [ ] Release-worthy", "- [x] Release-worthy");
    expect(check(release)).toContain("write a non-empty ready-to-publish release note");
    expect(check(release.replace("### Ready-to-publish note", "### Ready-to-publish note\n\n- Correct pool rotation when a participant is unavailable.")))
      .toEqual([]);
    expect(check(release.replace("### Ready-to-publish note", "### Ready-to-publish note\n<!-- a long hidden note is not a release note -->")))
      .toContain("write a non-empty ready-to-publish release note");
  });

  test("section names inside prose do not replace required headings", () => {
    expect(check(completed.replace("## Risk and rollout", "The template normally says ## Risk and rollout here.")))
      .toContain("include exactly one ## Risk and rollout section");
  });
});

describe("dependency automation exception", () => {
  const bot = { type: "Bot", login: "dependabot[bot]" };
  const files = [{ filename: "src/package.json" }, { filename: "src/bun.lock" }];
  const dependencyPr = { user: bot, body: "Dependency update", changed_files: files.length };

  test("known dependency updates are exempt without pretending that a bot signed", () => {
    expect(validatePullRequest(dependencyPr, files)).toEqual({ errors: [], dependencyAutomation: true });
  });

  test("Actions dependency updates retain the exception", () => {
    expect(validatePullRequest({ ...dependencyPr, changed_files: 1 }, [{ filename: ".github/workflows/build.yml" }]).dependencyAutomation)
      .toBe(true);
  });

  test("source changes in a dependency PR require a human PR", () => {
    expect(validatePullRequest(dependencyPr, [files[0], { filename: "src/src/main.tsx" }]).errors.length)
      .toBeGreaterThan(0);
  });

  test("unknown bot and human lookalikes are not exempt", () => {
    for (const user of [{ type: "Bot", login: "example-bot[bot]" }, { type: "User", login: "dependabot[bot]" }]) {
      expect(validatePullRequest({ ...dependencyPr, user }, files).errors.length).toBeGreaterThan(0);
    }
  });

  test("bots cannot satisfy the agreement merely by copying a completed template", () => {
    expect(validatePullRequest({ user: { type: "Bot", login: "example-bot[bot]" }, body: completed }).errors.length)
      .toBeGreaterThan(0);
  });

  test("empty or truncated file lists do not grant an exemption", () => {
    expect(validatePullRequest(dependencyPr, []).errors.length).toBeGreaterThan(0);
    expect(validatePullRequest(dependencyPr, files.slice(0, 1)).errors.length).toBeGreaterThan(0);
  });

  test("renaming a non-dependency file cannot hide it from the policy", () => {
    const renamed = [{ filename: "src/package.json", previous_filename: "LICENSE" }];
    expect(validatePullRequest({ ...dependencyPr, changed_files: 1 }, renamed).errors.length).toBeGreaterThan(0);
  });
});

describe("workflow integration", () => {
  const workflow = Bun.YAML.parse(readFileSync(new URL("../workflows/pr-metadata.yml", import.meta.url), "utf8"));
  const steps = workflow.jobs.metadata.steps;
  const script = steps.find((step) => step.uses?.startsWith("actions/github-script@"))?.with.script;
  const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;

  async function runWorkflow(pullRequest, files = []) {
    const failed = [];
    const info = [];
    const reads = [];
    const listFiles = () => {};
    const github = {
      rest: { pulls: {
        get: async (params) => {
          reads.push(params);
          return { data: pullRequest };
        },
        listFiles,
      } },
      paginate: async (method, params) => {
        expect(method).toBe(listFiles);
        reads.push(params);
        return files;
      },
    };
    const context = {
      repo: { owner: "example-owner", repo: "example-relay" },
      payload: { pull_request: { number: 17, body: completed } },
    };
    const core = { setFailed: (message) => failed.push(message), info: (message) => info.push(message) };
    const environment = { env: { GITHUB_WORKSPACE: new URL("../../", import.meta.url).href.replace(/\/$/, "") } };
    await new AsyncFunction("github", "context", "core", "process", script)(github, context, core, environment);
    return { failed, info, reads };
  }

  test("the policy runner checks out the base, disables stored credentials, and has no write access", () => {
    expect(workflow.on.pull_request_target.types).toContain("edited");
    expect(workflow.on.pull_request_target.types).toContain("synchronize");
    expect(workflow.on.pull_request).toBeUndefined();
    expect(workflow.permissions).toEqual({ contents: "read", "pull-requests": "read" });
    const checkout = steps.find((step) => step.uses?.startsWith("actions/checkout@"));
    expect(checkout.with.ref).toBe("${{ github.event.pull_request.base.sha }}");
    expect(checkout.with["persist-credentials"]).toBe(false);
    expect(steps.find((step) => step.name === "Test policy").run)
      .toBe("bun test ./.github/tools/pr-metadata.test.mjs");
    expect(script).not.toContain("${{");
  });

  test("current withdrawn consent fails even when the queued event still contains it", async () => {
    const result = await runWorkflow({ user: human, body: template });
    expect(result.failed).toHaveLength(1);
    expect(result.failed[0]).toContain("single checkbox");
    expect(result.reads).toEqual([{ owner: "example-owner", repo: "example-relay", pull_number: 17 }]);
  });

  test("a completed human PR passes without fetching its patch or executing its body", async () => {
    const result = await runWorkflow({ user: human, body: `${completed}\n\n"); throw new Error('untrusted PR text'); //` });
    expect(result.failed).toEqual([]);
    expect(result.info).toHaveLength(1);
    expect(result.reads).toHaveLength(1);
  });

  test("the bot exception fetches all file pages and checks their paths", async () => {
    const botPr = { user: { type: "Bot", login: "dependabot[bot]" }, body: null, changed_files: 1 };
    const accepted = await runWorkflow(botPr, [{ filename: "src/bun.lock" }]);
    expect(accepted.failed).toEqual([]);
    expect(accepted.reads[1]).toEqual({ owner: "example-owner", repo: "example-relay", pull_number: 17, per_page: 100 });
    const rejected = await runWorkflow(botPr, [{ filename: "src/src/main.tsx" }]);
    expect(rejected.failed).toHaveLength(1);
  });
});
