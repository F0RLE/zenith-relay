import { expect, test } from "bun:test";
import { measureCommand } from "../src/features/relay/api/commandPerformance";

test.each([
  ["update_local_routing", "command_save"],
  ["set_local_pool_membership", "command_save"],
  ["apply_local_configuration_preset", "command_switch"],
  ["select_local_account_route", "command_switch"],
  ["refresh_local_account_quota", "command_refresh"],
  ["refresh_all_local_account_quotas", "command_refresh"],
])("%s records one bounded IPC duration without command data", async (command, name) => {
  const samples: unknown[] = [];
  const result = { secret: "synthetic-private-result" };
  let clock = 5;
  let executions = 0;
  expect(await measureCommand(command, async () => {
    executions++;
    clock = 17.5;
    return result;
  }, async sample => { samples.push(sample); }, () => clock)).toBe(result);
  expect(executions).toBe(1);
  expect(samples).toEqual([{ name, durationMs: 12.5, context: `${command}:ok` }]);
  expect(JSON.stringify(samples)).not.toContain(result.secret);
});

test.each([
  "get_local_runtime_state", "record_local_performance_sample", "record_frontend_diagnostic",
  "unregistered_command", "refresh_private@example.test", "refresh_Invalid",
  `refresh_${"x".repeat(51)}`,
])("%s does not start a timer or a recursive telemetry call", async command => {
  const result = Promise.resolve({ value: true });
  const unexpected = () => { throw new Error("unused observer was called"); };
  expect(measureCommand(command, () => result, unexpected, unexpected)).toBe(result);
  await result;
});

test("a failed command retains its original rejection and reports the error outcome", async () => {
  const cause = { code: "conflict", message: "synthetic private failure" };
  const samples: unknown[] = [];
  let clock = 1;
  const result = measureCommand("update_local_source", async () => {
    clock = 26;
    throw cause;
  }, async sample => { samples.push(sample); }, () => clock);
  expect(await result.catch(error => error)).toBe(cause);
  expect(samples).toEqual([{
    name: "command_save", durationMs: 25, context: "update_local_source:error",
  }]);
  expect(JSON.stringify(samples)).not.toContain(cause.message);
});

test("a synchronous invoke failure is observed once without retry", async () => {
  const cause = new Error("synthetic invocation failure");
  const samples: unknown[] = [];
  let executions = 0;
  const result = measureCommand("refresh_local_source_data", () => {
    executions++;
    throw cause;
  }, async sample => { samples.push(sample); }, () => 1);
  expect(await result.catch(error => error)).toBe(cause);
  expect(executions).toBe(1);
  expect(samples).toEqual([{
    name: "command_refresh", durationMs: 0, context: "refresh_local_source_data:error",
  }]);
});

test("a pending diagnostic write does not block a successful command", async () => {
  const never = new Promise<void>(() => undefined);
  let writes = 0;
  const result = await measureCommand("select_local_account_route", async () => "done", () => {
    writes++;
    return never;
  }, () => 0);
  expect(result).toBe("done");
  expect(writes).toBe(1);
});

test.each([false, true])("telemetry failure preserves the original command outcome (reject=%s)", async reject => {
  const cause = new Error("synthetic command failure");
  for (const record of [
    () => { throw new Error("synthetic synchronous telemetry failure"); },
    () => Promise.reject(new Error("synthetic asynchronous telemetry failure")),
  ]) {
    const result = measureCommand("update_local_routing", () => reject
      ? Promise.reject(cause) : Promise.resolve("done"), record, () => 0);
    expect(await result.catch(error => error)).toBe(reject ? cause : "done");
  }
});

test.each([Number.NaN, Number.POSITIVE_INFINITY, -1, 600_001])("invalid elapsed time %s is discarded", async elapsed => {
  let clock = 0;
  let writes = 0;
  expect(await measureCommand("update_local_account", async () => {
    clock = elapsed;
    return "done";
  }, async () => { writes++; }, () => clock)).toBe("done");
  expect(writes).toBe(0);
});

test("a failed clock cannot block or change a command", async () => {
  const expected = Promise.resolve("done");
  expect(measureCommand("refresh_local_account_quota", () => expected,
    async () => { throw new Error("unused recorder"); },
    () => { throw new Error("clock unavailable"); })).toBe(expected);
  expect(await expected).toBe("done");
});

test("overlapping commands keep independent start times and completion order", async () => {
  const samples: unknown[] = [];
  let clock = 10;
  let finishFirst!: (value: string) => void;
  let finishSecond!: (value: string) => void;
  const record = async (sample: unknown) => { samples.push(sample); };
  const first = measureCommand("update_local_source", () => new Promise<string>(resolve => {
    finishFirst = resolve;
  }), record, () => clock);
  clock = 20;
  const second = measureCommand("refresh_local_account_quota", () => new Promise<string>(resolve => {
    finishSecond = resolve;
  }), record, () => clock);
  clock = 50;
  finishSecond("second");
  expect(await second).toBe("second");
  clock = 70;
  finishFirst("first");
  expect(await first).toBe("first");
  expect(samples).toEqual([
    { name: "command_refresh", durationMs: 30, context: "refresh_local_account_quota:ok" },
    { name: "command_save", durationMs: 60, context: "update_local_source:ok" },
  ]);
});
