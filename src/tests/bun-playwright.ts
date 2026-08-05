import { spawn } from "node:child_process";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createServer } from "node:net";
import {
  chromium,
  expect,
  test as base,
  type Browser,
  type Locator,
  type Page,
} from "@playwright/test";

type CdpTransport = {
  send(message: object): void;
  close(): void;
  onmessage?: (message: object) => void;
  onclose?: (reason?: string) => void;
};

type ManagedBrowser = {
  browser: Browser;
  close: () => Promise<void>;
};

async function stopBrowserProcess(
  child: ReturnType<typeof spawn>,
  profile: string,
): Promise<void> {
  if (child.exitCode === null) {
    child.kill();
    await Promise.race([
      new Promise<void>((resolve) => child.once("exit", () => resolve())),
      new Promise<void>((resolve) => setTimeout(resolve, 5_000)),
    ]);
  }

  let lastError: unknown;
  for (let attempt = 0; attempt < 50; attempt += 1) {
    try {
      await rm(profile, { recursive: true, force: true });
      return;
    } catch (error) {
      lastError = error;
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
  }
  throw lastError;
}

async function reservePort(): Promise<number> {
  return await new Promise((resolve, reject) => {
    const server = createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      if (!address || typeof address === "string") {
        server.close();
        reject(new Error("Could not reserve a local Playwright port."));
        return;
      }
      server.close((error) => (error ? reject(error) : resolve(address.port)));
    });
  });
}

async function waitForCdp(port: number): Promise<string> {
  const endpoint = `http://127.0.0.1:${port}/json/version`;
  const deadline = Date.now() + 60_000;
  let lastError = "unknown error";

  while (Date.now() < deadline) {
    try {
      const response = await fetch(endpoint, {
        signal: AbortSignal.timeout(1_000),
      });
      if (response.ok) {
        const payload = (await response.json()) as {
          webSocketDebuggerUrl?: string;
        };
        if (payload.webSocketDebuggerUrl) {
          return payload.webSocketDebuggerUrl;
        }
      }
    } catch (error) {
      lastError = error instanceof Error ? error.message : String(error);
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }

  throw new Error(`Chromium CDP endpoint did not start: ${lastError}`);
}

async function openBunWebSocket(endpoint: string): Promise<CdpTransport> {
  const socket = new WebSocket(endpoint);
  const transport: CdpTransport = {
    send(message) {
      socket.send(JSON.stringify(message));
    },
    close() {
      socket.close();
    },
  };

  await new Promise<void>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("CDP WebSocket timed out.")), 30_000);
    socket.onopen = () => {
      clearTimeout(timer);
      resolve();
    };
    socket.onerror = () => {
      clearTimeout(timer);
      reject(new Error("CDP WebSocket failed to open."));
    };
  });

  socket.onmessage = (event) => {
    transport.onmessage?.(JSON.parse(String(event.data)) as object);
  };
  socket.onclose = () => transport.onclose?.();
  return transport;
}

async function launchWindowsBrowser(): Promise<ManagedBrowser> {
  // Bun's Windows fd-3 transport is broken (oven-sh/bun#27977); use CDP until #35417 ships.
  const port = await reservePort();
  const profile = await mkdtemp(join(tmpdir(), "zenith-playwright-"));
  const child = spawn(
    chromium.executablePath(),
    [
      "--headless",
      "--no-sandbox",
      "--disable-gpu",
      "--disable-dev-shm-usage",
      "--disable-background-networking",
      "--disable-component-update",
      "--disable-default-apps",
      "--disable-extensions",
      "--disable-popup-blocking",
      "--disable-sync",
      "--hide-scrollbars",
      "--mute-audio",
      "--no-first-run",
      "--no-default-browser-check",
      "--no-startup-window",
      `--remote-debugging-port=${port}`,
      `--user-data-dir=${profile}`,
    ],
    { stdio: "ignore", windowsHide: true },
  );

  try {
    const endpoint = await waitForCdp(port);
    const transport = await openBunWebSocket(endpoint);
    const browser = await chromium.connectOverCDP(transport, { timeout: 60_000 });
    return {
      browser,
      close: async () => {
        try {
          await browser.close();
        } finally {
          await stopBrowserProcess(child, profile);
        }
      },
    };
  } catch (error) {
    await stopBrowserProcess(child, profile);
    throw error;
  }
}

export const test = base.extend<{ browser: Browser }>({
  browser: [
    async ({}, use) => {
      if (process.platform !== "win32") {
        const browser = await chromium.launch();
        try {
          await use(browser);
        } finally {
          await browser.close();
        }
        return;
      }

      const managed = await launchWindowsBrowser();
      try {
        await use(managed.browser);
      } finally {
        await managed.close();
      }
    },
    { scope: "worker" },
  ],
});

export { expect };
export type { Browser, Locator, Page };
