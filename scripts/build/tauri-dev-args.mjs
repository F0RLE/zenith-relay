/** Keep the pool serving agent requests until an explicit backend restart. */
export function tauriDevOptions(args = []) {
  const separator = args.indexOf("--");
  const cliArgs = separator === -1 ? args : args.slice(0, separator);
  const runnerArgs = separator === -1 ? [] : args.slice(separator);
  const watch = cliArgs.includes("--watch");
  if (watch && cliArgs.includes("--no-watch")) {
    throw new Error("Choose either --watch or --no-watch, not both.");
  }
  const forwarded = cliArgs.filter((arg) => arg !== "--watch" && arg !== "--no-watch");
  return {
    watch,
    help: cliArgs.includes("--help") || cliArgs.includes("-h"),
    args: [
      "dev", "--config", "src-tauri/tauri.conf.json",
      ...(!watch ? ["--no-watch"] : []),
      ...forwarded, ...runnerArgs,
    ],
  };
}
