import { describe, expect, test } from "bun:test";
import { createServer, request as httpRequest } from "node:http";
import { existsSync } from "node:fs";
import { isolatedEnvironment, qualifyEffortWire, runClient } from "./check-claude-effort-wire.mjs";

const model = { name: "claude-relay-0123-4567-89ab-cdef", behavesAs: "claude-opus-4-6", maxEffort: "max" };
const success = () => ({ code: 0, stdout: JSON.stringify({ type: "result", subtype: "success", is_error: false, result: "relay-contract-ok" }), stderr: "" });
const help = "--bare --tools --settings --setting-sources --strict-mcp-config --mcp-config --effort --no-session-persistence --output-format --system-prompt --disable-slash-commands";
const valueOf = (args, name) => args[args.indexOf(name) + 1];

function fixtureDriver({ changeEffort, changeModel, duplicate = false, complete = true, proxyAttempt = false, extraBody = {}, extraHeaders = {} } = {}) {
  const calls = [];
  const driver = async (input) => {
    calls.push(input);
    if (input.args[0] === "--version") return { code: 0, stdout: "2.1.309 (Claude Code)\n", stderr: "" };
    if (input.args[0] === "--help") return { code: 0, stdout: help, stderr: "" };
    if (proxyAttempt) {
      await new Promise((resolve, reject) => {
        const request = httpRequest(input.env.ANTHROPIC_BASE_URL, { method: "POST", path: "https://provider.invalid/v1/messages" }, (response) => {
          expect(response.statusCode).toBe(403);
          response.resume(); response.on("end", resolve);
        });
        request.on("error", reject); request.end();
      });
    }
    for (let index = 0; index < (duplicate ? 2 : 1); index++) {
      const response = await fetch(`${input.env.ANTHROPIC_BASE_URL}/v1/messages`, {
        method: "POST", headers: { "content-type": "application/json", ...extraHeaders },
        body: JSON.stringify({ ...extraBody, model: changeModel ?? valueOf(input.args, "--model"), output_config: { effort: changeEffort ?? valueOf(input.args, "--effort") }, stream: true }),
      });
      expect(response.status).toBe(200);
      expect(await response.text()).toContain("event: message_stop");
    }
    return complete ? success() : { code: 0, stdout: "incomplete", stderr: "" };
  };
  return { driver, calls };
}

async function withLoopbackGateway(handler, run) {
  const gateway = createServer(handler);
  try {
    await new Promise((resolve, reject) => {
      gateway.once("error", reject);
      gateway.listen(0, "127.0.0.1", resolve);
    });
    await run(`http://127.0.0.1:${gateway.address().port}`);
  } finally {
    gateway.closeAllConnections();
    if (gateway.listening) await new Promise((resolve) => gateway.close(resolve));
  }
}

describe("isolated installed-client wire qualification", () => {
  test("does not inherit provider credentials, profiles or permission overrides", () => {
    const env = isolatedEnvironment("fixture-root", "http://127.0.0.1:12345", {
      PATH: "fixture-path", ANTHROPIC_API_KEY: "private-key", ANTHROPIC_AUTH_TOKEN: "private-token",
      CLAUDE_CODE_OAUTH_TOKEN: "private-oauth", CLAUDE_CONFIG_DIR: "private-profile",
      CLAUDE_CODE_SKIP_PERMISSION_CHECKS: "1", ANTHROPIC_CUSTOM_HEADERS: "private-header",
    });
    expect(env.PATH).toBe("fixture-path");
    expect(env.ANTHROPIC_API_KEY).toBe("sk-ant-synthetic-loopback-contract");
    for (const key of ["ANTHROPIC_AUTH_TOKEN", "CLAUDE_CODE_OAUTH_TOKEN", "CLAUDE_CODE_SKIP_PERMISSION_CHECKS", "ANTHROPIC_CUSTOM_HEADERS"]) expect(env[key]).toBeUndefined();
    expect(env.CLAUDE_CONFIG_DIR).not.toBe("private-profile");
    expect(env.HTTP_PROXY).toBe(env.ANTHROPIC_BASE_URL);
    expect(env.NO_PROXY).toBe("127.0.0.1");
  });

  test("qualifies unchanged model and effort through a complete local SSE answer", async () => {
    const { driver, calls } = fixtureDriver();
    const result = await qualifyEffortWire({ cliPath: process.execPath, model, driver, withNativeControl: true });
    expect(result.ok).toBeTrue();
    expect(result.cases).toHaveLength(4);
    for (const call of calls.filter((call) => call.args[0] === "--bare")) {
      expect(valueOf(call.args, "--tools")).toBe("");
      expect(valueOf(call.args, "--max-turns")).toBe("1");
      expect(valueOf(call.args, "--setting-sources")).toBe("");
      expect(call.args.some((arg) => arg.includes("dangerously"))).toBeFalse();
      expect(existsSync(call.cwd)).toBeFalse();
      const settings = JSON.parse(valueOf(call.args, "--settings"));
      if (valueOf(call.args, "--model") === model.name) expect(settings.modelPicker.options).toEqual([{ model: model.name, behavesAs: model.behavesAs }]);
      else expect(settings).toEqual({});
    }
  });

  test("rejects silent effort downgrades", async () => {
    const { driver } = fixtureDriver({ changeEffort: "high" });
    const result = await qualifyEffortWire({ cliPath: process.execPath, model, driver });
    expect(result.ok).toBeFalse();
    expect(result.cases.every((row) => row.requests[0].effort === "high" && !row.ok)).toBeTrue();
  });

  test("rejects route substitution, duplicate requests and incomplete client results", async () => {
    for (const option of [{ changeModel: model.behavesAs }, { duplicate: true }, { complete: false }]) {
      const { driver } = fixtureDriver(option);
      expect((await qualifyEffortWire({ cliPath: process.execPath, model, efforts: ["max"], driver })).ok).toBeFalse();
    }
  });

  test("rejects outbound proxy attempts without forwarding them", async () => {
    const { driver } = fixtureDriver({ proxyAttempt: true });
    const result = await qualifyEffortWire({ cliPath: process.execPath, model, efforts: ["max"], driver });
    expect(result.blockedOutbound).toBe(1);
    expect(result.ok).toBeFalse();
  });

  test("fails closed when a client lacks isolation flags", async () => {
    const { driver, calls } = fixtureDriver();
    await expect(qualifyEffortWire({ cliPath: process.execPath, model, driver: async (input) => input.args[0] === "--help" ? { code: 0, stdout: "--effort", stderr: "" } : driver(input) })).rejects.toThrow("isolation");
    expect(calls).toHaveLength(1);
    expect(existsSync(calls[0].cwd)).toBeFalse();
  });

  test("rejects arbitrary settings and unsupported effort inputs before executing", async () => {
    const { driver, calls } = fixtureDriver();
    await expect(qualifyEffortWire({ cliPath: process.execPath, model: { ...model, hooks: {} }, driver })).rejects.toThrow("descriptor");
    await expect(qualifyEffortWire({ cliPath: process.execPath, model, efforts: ["ultra"], driver })).rejects.toThrow("effort");
    await expect(qualifyEffortWire({ cliPath: process.execPath, model: { ...model, maxEffort: "high" }, driver })).rejects.toThrow("cap");
    expect(calls).toHaveLength(0);
  });

  test("counts failed generation attempts before a successful response", async () => {
    const { driver } = fixtureDriver();
    const result = await qualifyEffortWire({ cliPath: process.execPath, model, efforts: ["max"], driver: async (input) => {
      if (input.args[0] === "--bare") {
        const rejected = await fetch(`${input.env.ANTHROPIC_BASE_URL}/v1/messages`, { method: "POST", body: "invalid JSON" });
        expect(rejected.status).toBe(400);
      }
      return driver(input);
    } });
    expect(result.ok).toBeFalse();
    expect(result.cases[0].messageRequests).toBe(2);
    expect(result.cases[0].invalidRequests).toBe(1);
  });

  test("bounds newly started child lifetime and output", async () => {
    const base = { cliPath: process.execPath, cwd: process.cwd(), env: isolatedEnvironment(process.cwd(), "http://127.0.0.1:1") };
    const timeout = await runClient({ ...base, args: ["-e", "setInterval(() => {}, 1000)"], timeoutMs: 100 });
    expect(timeout.failure).toBe("client_timeout");
    const output = await runClient({ ...base, args: ["-e", 'process.stdout.write("x".repeat(70000)); setInterval(() => {}, 1000)'], timeoutMs: 2000 });
    expect(output.failure).toBe("client_output_limit");
    expect(output.stdout.length + output.stderr.length).toBeLessThanOrEqual(64 * 1024);
  });

  test("gateway forwarding preserves the body and only passes allowlisted synthetic authorization", async () => {
    const observed = [];
    await withLoopbackGateway(async (request, response) => {
      const chunks = [];
      for await (const chunk of request) chunks.push(chunk);
      observed.push({ path: request.url, body: JSON.parse(Buffer.concat(chunks)), headers: request.headers });
      response.writeHead(200, { "content-type": "text/event-stream" });
      response.end('event: message_stop\ndata: {"type":"message_stop"}\n\n');
    }, async (gatewayOrigin) => {
      const { driver } = fixtureDriver({
        extraBody: { metadata: { synthetic_extension: [1, 2, 3] } },
        extraHeaders: { "anthropic-version": "2023-06-01", "anthropic-beta": "synthetic-beta", authorization: "Bearer rejected-synthetic-header", "x-api-key": "rejected-synthetic-key" },
      });
      const result = await qualifyEffortWire({ cliPath: process.execPath, model, efforts: ["max"], gatewayOrigin, driver });
      expect(result.ok).toBeTrue();
      expect(result.mode).toBe("relay_gateway");
    });
    expect(observed).toHaveLength(1);
    expect(observed[0].path).toBe("/v1/messages");
    expect(observed[0].body).toEqual({ metadata: { synthetic_extension: [1, 2, 3] }, model: model.name, output_config: { effort: "max" }, stream: true });
    expect(observed[0].headers["x-api-key"]).toBe("sk-ant-synthetic-loopback-contract");
    expect(observed[0].headers.authorization).toBeUndefined();
    expect(observed[0].headers["anthropic-version"]).toBe("2023-06-01");
    expect(observed[0].headers["anthropic-beta"]).toBe("synthetic-beta");
  });

  test("rejects non-loopback gateway destinations before executing the client", async () => {
    const { driver, calls } = fixtureDriver();
    for (const gatewayOrigin of ["https://127.0.0.1:12345", "http://localhost:12345", "http://provider.invalid:12345", "http://127.0.0.1:12345/v1", "http://key@127.0.0.1:12345", "http://127.0.0.1:12345/?redirect=1", "http://127.0.0.1:12345/#fragment"]) {
      await expect(qualifyEffortWire({ cliPath: process.execPath, model, gatewayOrigin, driver })).rejects.toThrow("loopback");
    }
    await expect(qualifyEffortWire({ cliPath: process.execPath, model, gatewayOrigin: "http://127.0.0.1:12345", withNativeControl: true, driver })).rejects.toThrow("generated alias");
    expect(calls).toHaveLength(0);
  });

  test("gateway errors and oversized replies fail qualification instead of inventing an acknowledgement", async () => {
    for (const oversized of [false, true]) {
      let forwarded = 0;
      await withLoopbackGateway((_request, response) => {
        forwarded++;
        response.writeHead(oversized ? 200 : 403, { "content-type": "application/json" });
        response.end(oversized ? "x".repeat(512 * 1024 + 1) : '{"error":"synthetic rejection"}');
      }, async (gatewayOrigin) => {
        const { driver: base } = fixtureDriver();
        const result = await qualifyEffortWire({ cliPath: process.execPath, model, efforts: ["max"], gatewayOrigin, driver: async (input) => {
          if (input.args[0] !== "--bare") return base(input);
          const response = await fetch(`${input.env.ANTHROPIC_BASE_URL}/v1/messages`, {
            method: "POST", body: JSON.stringify({ model: model.name, output_config: { effort: "max" }, stream: true }),
          });
          expect(response.status).toBe(oversized ? 400 : 403);
          await response.arrayBuffer();
          return { code: 1, stdout: "", stderr: "Synthetic client rejection" };
        } });
        expect(result.ok).toBeFalse();
        expect(result.cases[0].messageRequests).toBe(1);
        expect(result.cases[0].invalidRequests).toBe(oversized ? 1 : 0);
      });
      expect(forwarded).toBe(1);
    }
  });

  test("gateway redirects are not followed or exposed as client redirect targets", async () => {
    let forwarded = 0;
    await withLoopbackGateway((_request, response) => {
      forwarded++;
      response.writeHead(302, { location: "https://provider.invalid/v1/messages" });
      response.end();
    }, async (gatewayOrigin) => {
      const { driver: base } = fixtureDriver();
      const result = await qualifyEffortWire({ cliPath: process.execPath, model, efforts: ["max"], gatewayOrigin, driver: async (input) => {
        if (input.args[0] !== "--bare") return base(input);
        const response = await fetch(`${input.env.ANTHROPIC_BASE_URL}/v1/messages`, {
          method: "POST", redirect: "manual", body: JSON.stringify({ model: model.name, output_config: { effort: "max" } }),
        });
        expect(response.status).toBe(302);
        expect(response.headers.get("location")).toBeNull();
        await response.arrayBuffer();
        return { code: 1, stdout: "", stderr: "Synthetic redirect rejection" };
      } });
      expect(result.ok).toBeFalse();
    });
    expect(forwarded).toBe(1);
  });

  test("a stalled loopback gateway is bounded by the qualification timeout", async () => {
    await withLoopbackGateway((request) => request.resume(), async (gatewayOrigin) => {
      const { driver: base } = fixtureDriver();
      const result = await qualifyEffortWire({ cliPath: process.execPath, model, efforts: ["max"], gatewayOrigin, timeoutMs: 1000, driver: async (input) => {
        if (input.args[0] !== "--bare") return base(input);
        const response = await fetch(`${input.env.ANTHROPIC_BASE_URL}/v1/messages`, {
          method: "POST", body: JSON.stringify({ model: model.name, output_config: { effort: "max" } }),
        });
        expect(response.status).toBe(400);
        await response.arrayBuffer();
        return { code: 1, stdout: "", stderr: "Synthetic timeout rejection" };
      } });
      expect(result.ok).toBeFalse();
      expect(result.cases[0].invalidRequests).toBe(1);
    });
  });
});
