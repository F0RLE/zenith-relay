import { expect, test } from "bun:test";
import { operationJobs, parseOperationsEvents, parseOperationsStatus } from "../../src/api/ownerOperations";

const time = 1780000000000;
const jobs = operationJobs.map((job, i) => ({ job, state: i === 0 ? "unobserved" : "stale",
  observed_at_ms: i === 0 ? null : time - 100000 }));
const status = { version: 1, as_of_ms: time, jobs };

test("operations keeps unavailable observations distinct from successful passes", () => {
  expect(parseOperationsStatus(status).jobs.map(v => v.state)).toEqual(["unobserved", ...Array(5).fill("stale")]);
  expect(() => parseOperationsStatus({ ...status, jobs: jobs.slice(1) })).toThrow();
  expect(() => parseOperationsStatus({ ...status, jobs: [jobs[1]!, ...jobs.slice(1)] })).toThrow();
  expect(() => parseOperationsStatus({ ...status, jobs: [{ ...jobs[0], state: "succeeded" }, ...jobs.slice(1)] })).toThrow();
});

const event = { id: "9007199254740993", job: "request_reconciliation", code: "job_failed",
  first_at_ms: time - 2000, last_at_ms: time, observations: 2 };
const page = { version: 1, partial: true, items: [event], next_before: null };

test("operations pages require canonical ordered cursors and real saved counts", () => {
  expect(parseOperationsEvents({ ...page, items: [] })).toEqual({ items: [], partial: true, next: null });
  expect(parseOperationsEvents(page, "9007199254740994").items[0]?.count).toBe(2);
  expect(() => parseOperationsEvents(page, event.id)).toThrow();
  expect(() => parseOperationsEvents({ ...page, items: [event, event] })).toThrow();
  expect(() => parseOperationsEvents({ ...page, items: [{ ...event, code: "job_recovered" }] })).toThrow();
  expect(() => parseOperationsEvents({ ...page, items: [{ ...event, observations: 0 }] })).toThrow();
  expect(() => parseOperationsEvents({ ...page, next_before: event.id })).toThrow();
});
