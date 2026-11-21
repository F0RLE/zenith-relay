import { expect } from "@playwright/test";

export function loginFixture(base, origin) {
  const begin = (client, headers = {}) => client.post(base + "/begin", { headers: { Origin: origin, ...headers } });
  const authorization = async (client, start) => {
    expect(start.status()).toBe(200);
    const url = (await start.json()).data.authorization_url;
    const r = await client.get(url, { maxRedirects: 0 });
    expect(r.status()).toBe(303);
    const target = r.headers().location;
    if (new URL(target).origin !== origin) throw new Error("Unexpected fixture callback origin");
    return target;
  };
  const prepare = async client => authorization(client, await begin(client));
  const complete = (client, target) => client.get(target, { maxRedirects: 0 });
  return { begin, authorization, prepare, complete };
}

const denied = response => {
  expect(response.status()).toBe(303);
  expect(response.headers().location).toBe("/login#denied");
  expect(response.headers()["set-cookie"] === undefined).toBe(true);
};

export async function checkLoginBudgets({ flow, idp, pageFor, login }) {
  const page = await pageFor("outsider"), independent = await pageFor("outsider");
  try {
    const first = await flow.prepare(page.request);
    for (let n = 0; n < 4; n++) expect((await flow.begin(page.request)).status()).toBe(200);
    const last = await flow.prepare(page.request);
    const before = { ...idp.counts };
    const limited = await flow.begin(page.request, {
      Forwarded: "for=198.51.100.20", "X-Forwarded-For": "203.0.113.20",
    });
    expect(limited.status()).toBe(429);
    expect((await limited.json()).error.code).toBe("customer_rate_limited");
    const retry = Number(limited.headers()["retry-after"]);
    expect(Number.isInteger(retry) && retry >= 1 && retry <= 20).toBe(true);
    expect(limited.headers()["set-cookie"] === undefined).toBe(true);
    await login(page);
    await expect(page.getByRole("alert")).toHaveText("Too many sign-in attempts. Try again shortly.");
    denied(await flow.complete(page.request, first));
    expect(idp.counts).toEqual(before);
    denied(await flow.complete(page.request, last));
    denied(await flow.complete(independent.request, await flow.prepare(independent.request)));
    expect(idp.counts.exchanges).toBe(before.exchanges + 2);
  } finally {
    await Promise.all([page.context().close(), independent.context().close()]);
  }

  const pages = [];
  try {
    for (let n = 0; n < 5; n++) pages.push(await pageFor("outsider"));
    const targets = [];
    for (const page of pages) targets.push(await flow.prepare(page.request));
    const before = idp.counts.exchanges;
    const resume = idp.holdTokenReplies();
    const active = Promise.all(pages.slice(0, 4).map((p, i) => flow.complete(p.request, targets[i])));
    let finished;
    try {
      await expect.poll(() => idp.counts.exchanges, { timeout: 3000 }).toBe(before + 4);
      const busy = await flow.complete(pages[4].request, targets[4]);
      expect(busy.status()).toBe(303);
      expect(busy.headers().location).toBe("/login#busy");
      expect(busy.headers()["set-cookie"] === undefined).toBe(true);
      denied(await flow.complete(pages[0].request, targets[0]));
      expect(idp.counts.exchanges).toBe(before + 4);
    } finally {
      resume();
      finished = await active;
    }
    finished.forEach(denied);
    denied(await flow.complete(pages[4].request, targets[4]));
    expect(idp.counts.exchanges).toBe(before + 5);
  } finally {
    await Promise.all(pages.map(p => p.context().close()));
  }
}

export async function checkLoginScopeExpiry({ flow, idp, pageFor, customer, expire }) {
  const page = await pageFor("bob");
  try {
    const stale = await flow.prepare(customer), inFlight = await flow.prepare(page.request);
    const before = { ...idp.counts };
    const resume = idp.holdTokenReplies();
    const completion = flow.complete(page.request, inFlight);
    try {
      await expect.poll(() => idp.counts.exchanges, { timeout: 3000 }).toBe(before.exchanges + 1);
      await expire();
      expect((await flow.begin(customer)).status()).toBe(503);
      denied(await flow.complete(customer, stale));
      expect(idp.counts.exchanges).toBe(before.exchanges + 1);
      expect(idp.counts.keyReads).toBe(before.keyReads);
    } finally {
      resume();
      denied(await completion);
    }
    expect(idp.counts.keyReads).toBe(before.keyReads + 1);
  } finally { await page.context().close(); }
}
