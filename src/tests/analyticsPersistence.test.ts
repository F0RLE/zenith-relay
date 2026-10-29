import { describe, expect, test } from "bun:test";
import { createAnalyticsPersistence } from "../src/features/relay/pages/overview/analyticsPersistence";

function scheduler() {
  const tasks: Array<{ run: () => void; cancelled: boolean }> = [];
  return {
    tasks,
    schedule: (run: () => void) => {
      const task = { run, cancelled: false };
      tasks.push(task);
      return () => { task.cancelled = true; };
    },
  };
}

describe("deferred analytics persistence", () => {
  test("a burst performs no synchronous writes and persists the latest state once", () => {
    const queue = scheduler();
    let current = 0;
    const written: number[] = [];
    const persistence = createAnalyticsPersistence(() => written.push(current), queue.schedule);
    for (current = 1; current <= 20; current += 1) persistence.request();
    current = 20;
    expect(written).toEqual([]);
    expect(queue.tasks).toHaveLength(1);
    queue.tasks[0].run();
    expect(written).toEqual([20]);
    persistence.flush();
    expect(written).toEqual([20]);
  });

  test("lifecycle flush cancels the timer and stale callbacks cannot write newer state", () => {
    const queue = scheduler();
    let current = 1;
    const written: number[] = [];
    const persistence = createAnalyticsPersistence(() => written.push(current), queue.schedule);
    persistence.request();
    persistence.flush();
    expect(queue.tasks[0].cancelled).toBe(true);
    expect(written).toEqual([1]);
    current = 2;
    persistence.request();
    queue.tasks[0].run();
    expect(written).toEqual([1]);
    queue.tasks[1].run();
    expect(written).toEqual([1, 2]);
  });

  test("a failed optional writer does not leave future persistence blocked", () => {
    const queue = scheduler();
    let attempts = 0;
    const persistence = createAnalyticsPersistence(() => {
      attempts += 1;
      if (attempts === 1) throw new Error("Synthetic storage failure");
    }, queue.schedule);
    persistence.request();
    expect(() => queue.tasks[0].run()).toThrow("Synthetic storage failure");
    persistence.request();
    queue.tasks[1].run();
    expect(attempts).toBe(2);
  });
});
