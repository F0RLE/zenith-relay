import { expect } from "@playwright/test";
import { createHash, createHmac, randomBytes } from "node:crypto";
import { join } from "node:path";
import { readFixture, writeFixture } from "../../test-support/files.mjs";
import { checkWebModelQualification } from "./web-model-qualification.mjs";
import { supplyConfirmation } from "./supply-confirmation.mjs";

const name = "Synthetic web connection";
const declared = "fixture-text";

export async function checkWebSourceIntake(page, origin, exchange) {
  const { key, proxy, endpoint } = JSON.parse(readFixture(exchange, "web-model-route"));
  function redacted(value) {
    const encoded = JSON.stringify(value);
    expect(!encoded.includes(key) && !encoded.includes(proxy) && !encoded.includes(new URL(proxy).password)).toBe(true);
  }
  const api = origin + "/api/hosted/v1/";
  const overview = api + "admin/overview";
  const resource = api + "admin/connections/intake";
  const options = resource + "/options";
  const target = api + "transfer/intake/";
  const auth = (op, token, site = origin) => ({
    url: target + op, headers: { Origin: site, Authorization: "Bearer " + token, "Content-Type": "application/octet-stream" },
  });
  const readOwner = async () => (await (await page.request.get(overview)).json()).data;
  expect((await readOwner()).web_api_intake).toBe(true);
  const proof = supplyConfirmation(page, origin, api, async () => (await readOwner()).csrf_token);
  const guest = await page.context().browser().newContext();
  try {
    const token = randomBytes(32).toString("hex"), op = crypto.randomUUID();
    const post = auth(op, token);
    expect((await guest.request.post(post.url, { headers: post.headers, data: key + "\n" })).status()).toBe(401);
    expect((await guest.request.get(resource + "/" + op)).status()).toBe(401);
    expect((await proof.post(options, {}, { client: guest.request })).status()).toBe(401);
    expect((await proof.post(options, {}, { token: "0".repeat(64) })).status()).toBe(403);

    const add = page.getByRole("button", { name: "Add API connection", exact: true });
    const form = page.getByRole("dialog", { name: "Add API connection" });
    for (const [width, theme] of [[1024, "light"], [320, "dark"]]) {
      await page.setViewportSize({ width, height: width === 320 ? 640 : 760 });
      await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" });
      await page.reload();
      await expect(page.locator(".hosted-entry")).toHaveAttribute("data-theme", theme);
      await add.press("Enter");
      await expect(form.getByLabel("Name", { exact: true })).toBeFocused();
      await expect(form.getByRole("button", { name: "Save", exact: true })).toBeDisabled();
      await form.getByRole("combobox", { name: "Route" }).click();
      await form.getByRole("option", { name: "Via proxy" }).click();
      await expect(form.getByLabel("Proxy URL")).toHaveAttribute("type", "password");
      expect(await form.evaluate(node => {
        const box = node.getBoundingClientRect();
        return box.left >= 0 && box.right <= innerWidth && box.top >= 0 && box.bottom <= innerHeight
          && document.querySelector(".relay-content").scrollWidth <= document.querySelector(".relay-content").clientWidth;
      })).toBe(true);
      if (process.env.RELAY_TEST_SOURCE_INTAKE_CAPTURE)
        await page.screenshot({ path: join(process.env.RELAY_TEST_SOURCE_INTAKE_CAPTURE, `api-add-${width}-${theme}.png`) });
      if (width !== 320) { await page.keyboard.press("Escape"); await expect(add).toBeFocused(); }
    }

    await form.getByLabel("Name", { exact: true }).fill(name);
    const baseUrl = form.getByLabel("Endpoint URL");
    await baseUrl.fill(`${endpoint}?secret=fixture`);
    await form.getByLabel("Provider API key").fill(key);
    await form.getByLabel("Proxy URL").fill(proxy);
    await expect(form.getByRole("alert")).toHaveText(/No credentials or parameters/);
    await expect(form.getByRole("button", { name: "Save", exact: true })).toBeDisabled();
    await baseUrl.fill(endpoint);
    await form.getByRole("button", { name: "Models", exact: true }).click();
    await form.getByLabel("Model ID").fill(declared);
    await form.getByLabel("Model ID").press("Enter");
    await expect(form.getByText("Awaiting verification", { exact: true })).toBeVisible();

    let operation, source, submissions = 0;
    const draft = page.waitForRequest(r => r.url() === options && r.method() === "POST");
    await page.route(target + "*", async route => {
      submissions++;
      const request = route.request();
      operation = new URL(request.url()).pathname.split("/").at(-1);
      const headers = await request.allHeaders();
      expect(headers.origin).toBe(origin);
      expect(headers.cookie).toBeUndefined();
      const bad = await guest.request.post(request.url(), {
        headers: { Origin: origin, Authorization: headers.authorization, "Content-Type": "application/octet-stream" },
        data: key + "\n",
      });
      expect(bad.status()).toBe(409);
      redacted(await bad.json());
      // route.fetch would attach the signed-in context's cookies despite credentials: "omit".
      const response = await guest.request.post(request.url(), {
        headers: { Origin: origin, Authorization: headers.authorization, "Content-Type": headers["content-type"] },
        data: request.postDataBuffer(), maxRedirects: 0,
      });
      try {
        expect(response.status()).toBe(200);
        const reply = await response.json();
        redacted(reply);
        expect(reply.data).toMatchObject({ state: "saved", operation_id: operation });
        source = reply.data.source_id;
      } finally {
        await response.dispose();
        await route.abort("failed");
      }
    });
    await form.getByRole("button", { name: "Save", exact: true }).click();
    const confirmation = (await draft).postDataJSON();
    redacted(confirmation);
    expect(confirmation).toMatchObject({ label: name, endpoint, declared_models: [declared], proxied: true });
    await expect(page.getByRole("status").filter({ hasText: "Save not confirmed." })).toBeVisible();
    const check = page.getByRole("button", { name: "Check connection save", exact: true });
    await page.reload();
    await check.click();
    const recovery = page.getByRole("dialog", { name: "Check connection save" });
    await expect(recovery.locator("input")).toHaveCount(0);
    await recovery.getByRole("button", { name: "Check result" }).click();
    await expect(recovery).toBeHidden();
    expect(confirmation.operation_id).toBe(operation);
    expect(submissions).toBe(1);
    const row = page.getByRole("article", { name, exact: true });
    await expect(row.getByText("Proxy assigned", { exact: true })).toBeVisible();
    await expect(row.getByText("Outside pool", { exact: true })).toBeVisible();
    await row.getByRole("button", { name: "Awaiting verification" }).click();
    const models = page.getByRole("dialog", { name: "Models", exact: true });
    await expect(models.getByText(declared, { exact: true })).toBeVisible();
    await expect(models.getByText("No current model checks", { exact: true })).toBeVisible();
    await page.keyboard.press("Escape");
    const inventory = (await (await page.request.get(api + "admin/connections?view=api")).json()).data;
    redacted(inventory);
    expect(inventory.items.filter(item => item.id === source)).toEqual([{
      id: source, label: name, endpoint, declared_models: [declared], proxy_bound: true,
      verified_models: [], credential_state: "stored", serving: { revision: "0", state: "enabled", in_pool: false, draining: false },
    }]);
    await checkWebModelQualification({ page, guest, api, exchange, source, row, proof, declared, redacted });
    const { checkDesktopPoolReview } = await import("./desktop-pool-review.mjs");
    await checkDesktopPoolReview(page, exchange, source, name, declared);

    const verifier = randomBytes(32);
    const body = key + "\n";
    const rejected = { ...confirmation, operation_id: crypto.randomUUID(), proxied: false, declared_models: [],
      verifier_digest: createHash("sha256").update(verifier).digest("hex"),
      payload_mac: createHmac("sha256", verifier).update(body).digest("hex") };
    expect((await proof.post(options, { ...rejected, endpoint: "http://169.254.169.254/v1" })).status()).toBe(400);
    expect((await proof.post(options, { ...rejected, declared_models: ["a", "A"] })).status()).toBe(400);
    const challenge = await proof.prepare(resource, rejected);
    const assert = await proof.assertion(challenge);
    expect((await proof.post(proof.path(challenge), assert)).status()).toBe(200);
    const matching = auth(rejected.operation_id, verifier.toString("hex"));
    const wrongOrigin = auth(rejected.operation_id, verifier.toString("hex"), "https://other.invalid");
    expect((await guest.request.post(wrongOrigin.url, { headers: wrongOrigin.headers, data: body })).status()).toBe(401);
    await page.setViewportSize({ width: 1024, height: 760 });
    await page.getByRole("button", { name: "Sign out" }).click();
    await expect(page).toHaveURL(origin + "/admin/login");
    expect((await guest.request.post(matching.url, { headers: matching.headers, data: body })).status()).toBe(401);
    writeFixture(exchange, "web-source", source);
  } finally { await guest.close(); }
}
