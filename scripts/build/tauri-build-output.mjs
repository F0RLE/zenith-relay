import { spawn } from "node:child_process";
import { closeSync, mkdirSync, openSync } from "node:fs";
import { dirname } from "node:path";
import { stripVTControlCharacters } from "node:util";
import { readDescriptorText } from "../lib/checked-file.mjs";

export function conciseBuildOutput(output, failed = false) {
  const lines = stripVTControlCharacters(output).split(/\r?\n/);
  if (!failed) {
    let diagnostic = false;
    return lines.filter(line => {
      if (/^\s*(?:warning[:\[]|error[:\[]|WARN\b|\(!\))/.test(line)) diagnostic = true;
      if (diagnostic) {
        if (!line.trim()) diagnostic = false;
        return true;
      }
      return /^\s*(?:Finished `|Built application at:|✓ built|initialJavaScript:|initialCss:|applicationAssets:)/.test(line);
    }).join("\n").trim();
  }
  let backtrace = false;
  return lines.filter(line => {
    if (line === "stack backtrace:") {
      backtrace = true;
      return false;
    }
    if (backtrace && /^\s*(?:\d+:\s|at\s)/.test(line)) return false;
    backtrace = false;
    return !/^(?:dist\/\S+\s+[\d.]+\s+[kMG]?B\b|\[PLUGIN_TIMINGS\]|\d+ callbacks are listed|the time inside one call|To find their cost|See https:\/\/rolldown\.rs|\s+- plugin vite:|note: compiler flags:|\s*process didn't exit successfully:)/.test(line);
  }).join("\n").trim();
}

export async function runLoggedTauriBuild(invocation, options, {
  logPath,
  progressIntervalMs = 30_000,
  log = console.log,
  error = console.error,
}) {
  mkdirSync(dirname(logPath), { recursive: true });
  const output = openSync(logPath, "w+");
  const started = performance.now();
  let progress;
  let result;
  try {
    log(`[build] Full log: ${logPath}`);
    progress = progressIntervalMs > 0
      ? setInterval(() => log(`[build] Running: ${Math.round((performance.now() - started) / 1000)}s elapsed.`), progressIntervalMs)
      : null;
    progress?.unref();
    result = await new Promise(resolve => {
      let child;
      try {
        child = spawn(invocation.command, invocation.args, {
          ...options,
          stdio: ["inherit", output, output],
        });
      } catch (failure) {
        resolve({ error: failure });
        return;
      }
      let spawnError;
      child.once("error", failure => { spawnError = failure; });
      child.once("close", (status, signal) => resolve({ status, signal, error: spawnError }));
    });
    // Read the same opened object, even if its pathname was replaced during the build.
    const failed = Boolean(result.error || result.signal || result.status !== 0);
    const summary = conciseBuildOutput(readDescriptorText(output), failed);
    if (summary) (failed ? error : log)(summary);
    return result;
  } finally {
    if (progress) clearInterval(progress);
    closeSync(output);
  }
}
