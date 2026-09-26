import { expect, test } from "bun:test";
import { platformFromUserAgent } from "../src/platform/desktop";

test("initial title bar follows the host while the native platform command loads", () => {
  expect(platformFromUserAgent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")).toBe("windows");
  expect(platformFromUserAgent("Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) AppleWebKit/605.1.15")).toBe("macos");
  expect(platformFromUserAgent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36")).toBe("linux");
  expect(platformFromUserAgent("Mozilla/5.0 (Unknown OS) AppleWebKit/537.36")).toBe("unknown");
});
