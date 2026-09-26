const agreementConfirmation = "I have read and agree to the [Contributor Agreement v1.0](https://github.com/F0RLE/zenith-relay/blob/release/1.1.3/CONTRIBUTOR_LICENSE_AGREEMENT.md), including its copyright assignment. If this PR is accepted, I assign to F0RLE the copyright I own in my original changes and confirm that I have permission to submit them.";

const requiredHeadings = [
  "Contributor Agreement",
  "Summary",
  "User-visible changes",
  "Release Notes",
  "Validation",
  "Risk and rollout",
];

const dependencyFiles = new Set([
  "src/package.json",
  "src/bun.lock",
  "Cargo.toml",
  "Cargo.lock",
  "crates/relay-core/Cargo.toml",
  "crates/relay-core/Cargo.lock",
  "relay-server/Cargo.toml",
  "relay-server/Cargo.lock",
  "src-tauri/Cargo.toml",
  "src-tauri/Cargo.lock",
]);

function isDependencyFile(path) {
  return dependencyFiles.has(path) || /^\.github\/workflows\/[^/]+\.ya?ml$/.test(path);
}

function isDependencyAutomation(pullRequest, files) {
  return pullRequest.user?.type === "Bot"
    && pullRequest.user.login === "dependabot[bot]"
    && files.length > 0
    && files.length === pullRequest.changed_files
    && files.every((file) => isDependencyFile(file.filename)
      && (!file.previous_filename || isDependencyFile(file.previous_filename)));
}

// Consent in comments, code examples, or a different section must not count.
function visibleLines(body) {
  const lines = body.replace(/<!--[^]*?(?:-->|$)/g, "").split(/\r?\n/);
  const visible = [];
  let fence;
  for (const line of lines) {
    const marker = line.match(/^ {0,3}(`{3,}|~{3,})(.*)$/);
    if (fence) {
      if (marker && marker[1][0] === fence[0]
        && marker[1].length >= fence.length && marker[2].trim() === "") {
        fence = undefined;
      }
    } else if (marker) {
      fence = marker[1];
    } else if (!/^(?: {4}|\t)/.test(line)) {
      visible.push(line);
    }
  }
  return visible;
}

function readSections(body) {
  const sections = new Map();
  let current;
  for (const line of visibleLines(body)) {
    const heading = line.match(/^ {0,3}##[ \t]+(.+?)[ \t]*#*[ \t]*$/);
    if (heading) {
      const name = heading[1].toLowerCase();
      current = [];
      const matches = sections.get(name) ?? [];
      matches.push(current);
      sections.set(name, matches);
    } else {
      current?.push(line);
    }
  }
  return sections;
}

export function validatePullRequest(pullRequest, files = []) {
  if (isDependencyAutomation(pullRequest, files)) {
    return { errors: [], dependencyAutomation: true };
  }
  if (pullRequest.user?.type !== "User") {
    return {
      errors: ["automated accounts cannot sign for contributors; use a human-authored PR with the rights holders' consent"],
      dependencyAutomation: false,
    };
  }

  const sections = readSections(pullRequest.body ?? "");
  const errors = requiredHeadings
    .filter((heading) => sections.get(heading.toLowerCase())?.length !== 1)
    .map((heading) => `include exactly one ## ${heading} section`);
  const section = (heading) => sections.get(heading.toLowerCase())?.[0] ?? [];
  const confirmations = section("Contributor Agreement")
    .map((line) => line.match(/^ {0,3}- \[([ xX])\] (.+?)\s*$/))
    .filter(Boolean);

  const consent = confirmations.filter((match) => match[2] === agreementConfirmation);
  if (consent.length !== 1 || consent[0][1].toLowerCase() !== "x") {
    errors.push("read the linked Contributor Agreement and check its single checkbox yourself, keeping the original wording");
  }

  const releaseLines = section("Release Notes");
  const releaseSection = releaseLines.join("\n");
  const releaseWorthy = /^ {0,3}-\s*\[[xX]\]\s*Release-worthy\b/im.test(releaseSection);
  const noReleaseNote = /^ {0,3}-\s*\[[xX]\]\s*No release note\b/im.test(releaseSection);
  if (releaseWorthy === noReleaseNote) {
    errors.push("select exactly one Release Notes checkbox");
  }
  const noteStart = releaseLines.findIndex((line) => /^ {0,3}###[ \t]+Ready-to-publish note\s*$/i.test(line));
  const noteEnd = releaseLines.findIndex((line, index) => index > noteStart && /^ {0,3}###[ \t]/.test(line));
  const note = noteStart < 0 ? "" : releaseLines.slice(noteStart + 1, noteEnd < 0 ? undefined : noteEnd).join("\n");
  const noteText = note.replace(/-\s*\[[ xX]\].*/g, "").replace(/[_* >#-]/g, "").trim();
  if (releaseWorthy && noteText.length < 12) {
    errors.push("write a non-empty ready-to-publish release note");
  }

  return { errors, dependencyAutomation: false };
}
