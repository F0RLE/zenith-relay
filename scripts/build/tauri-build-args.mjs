export function tauriBuildOptions(args) {
  const separator = args.indexOf("--");
  let compact = false;
  const forwarded = args.filter((arg, index) => {
    if (arg === "--compact" && (separator < 0 || index < separator)) {
      compact = true;
      return false;
    }
    return true;
  });
  return { args: forwarded, compact };
}

/** Keep Tauri options ahead of Cargo's separator, including unsigned local builds. */
export function tauriBuildArgs(args, { signingKeyPresent = false } = {}) {
  const separator = args.indexOf("--");
  const tauriArgs = separator < 0 ? args : args.slice(0, separator);
  const cargoArgs = separator < 0 ? [] : args.slice(separator + 1);
  const result = ["build", "--config", "src-tauri/tauri.conf.json", ...tauriArgs];
  if (!signingKeyPresent) {
    result.push("--config", JSON.stringify({ bundle: { createUpdaterArtifacts: false } }));
  }
  // Building must not silently rewrite the committed dependency resolution.
  const locked = cargoArgs.includes("--locked") || cargoArgs.includes("--frozen");
  return [...result, "--", ...(locked ? [] : ["--locked"]), ...cargoArgs];
}
