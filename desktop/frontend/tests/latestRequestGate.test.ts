import { describe, expect, test } from "bun:test";
import { LatestRequestGate } from "../src/features/relay/state/latestRequestGate";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((complete) => { resolve = complete; });
  return { promise, resolve };
}

describe("latest request gate", () => {
  test("commits only the newest result when requests finish out of order", async () => {
    const gate = new LatestRequestGate();
    const first = deferred<number>();
    const second = deferred<number>();
    const committed: number[] = [];
    const firstRun = gate.run(() => first.promise, (value) => committed.push(value));
    const secondRun = gate.run(() => second.promise, (value) => committed.push(value));

    second.resolve(2);
    expect(await secondRun).toBe(2);
    first.resolve(1);
    expect(await firstRun).toBe(1);
    expect(committed).toEqual([2]);
  });

  test("invalidates an in-flight result when the runtime mode changes", async () => {
    const gate = new LatestRequestGate();
    const pending = deferred<string>();
    const committed: string[] = [];
    const run = gate.run(() => pending.promise, (value) => committed.push(value));
    gate.invalidate();
    pending.resolve("stale");
    expect(await run).toBe("stale");
    expect(committed).toEqual([]);
  });
});
