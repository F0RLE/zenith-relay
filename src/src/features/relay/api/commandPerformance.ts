type CommandMetric = "command_save" | "command_switch" | "command_refresh";

type CommandSample = { name: CommandMetric; durationMs: number; context: string };

function commandMetric(command: string): CommandMetric | undefined {
  if (command.length > 58 || !/^[a-z][a-z0-9_]*$/.test(command)) return undefined;
  const prefix = command.split("_", 1)[0];
  switch (prefix) {
    case "create": case "update": case "save": case "set": case "rename": case "rotate":
      return "command_save";
    case "apply": case "select": case "activate": case "switch":
      return "command_switch";
    case "refresh":
      return "command_refresh";
    default:
      return undefined;
  }
}

/** Observe one IPC round trip. Arguments and returned values never enter telemetry. */
export function measureCommand<T>(
  command: string,
  execute: () => Promise<T>,
  record: (sample: CommandSample) => Promise<unknown>,
  now: () => number = () => performance.now(),
): Promise<T> {
  const name = commandMetric(command);
  if (!name) return execute();
  let started: number;
  try { started = now(); } catch { return execute(); }
  const finish = (outcome: "ok" | "error") => {
    try {
      const durationMs = now() - started;
      if (!Number.isFinite(durationMs) || durationMs < 0 || durationMs > 600_000) return;
      void record({ name, durationMs, context: `${command}:${outcome}` }).catch(() => undefined);
    } catch {
      // Optional telemetry must not change the command's result or rejection.
    }
  };
  let result: Promise<T>;
  try { result = execute(); } catch (cause) {
    finish("error");
    return Promise.reject(cause);
  }
  return result.then(value => {
    finish("ok");
    return value;
  }, cause => {
    finish("error");
    throw cause;
  });
}
