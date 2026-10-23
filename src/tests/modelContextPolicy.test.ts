import { expect, test } from "bun:test";
import { contextWindowTokens, customContextMaximum, knownContextMaximum } from "../src/features/relay/pages/pool/modelContextPolicy";

test("short is 25 percent of each real model maximum, not a global token cap", () => {
  for (const [maximum, short] of [[1_000_000, 250_000], [1_050_000, 262_500], [200_000, 50_000], [32_000, 8_000], [512, 512]]) {
    expect(contextWindowTokens({ mode: "short" }, maximum)).toBe(short);
    expect(contextWindowTokens({ mode: "extended" }, maximum)).toBe(maximum);
  }
});

test("unknown or invalid maxima stay unknown instead of becoming 128k", () => {
  for (const maximum of [undefined, null, 0, -1, NaN, Infinity, 1.5]) {
    expect(knownContextMaximum(maximum)).toBeUndefined();
    expect(contextWindowTokens({ mode: "short" }, maximum ?? undefined)).toBeUndefined();
    expect(contextWindowTokens({ mode: "extended" }, maximum ?? undefined)).toBeUndefined();
  }
});

test("published short and long windows override the percentage fallback", () => {
  expect(contextWindowTokens({ mode: "short" }, 1_050_000, 272_000)).toBe(272_000);
  expect(contextWindowTokens({ mode: "extended" }, 1_050_000, 272_000)).toBe(1_050_000);
  expect(contextWindowTokens({ mode: "short" }, undefined, 200_000)).toBe(200_000);
  expect(contextWindowTokens({ mode: "extended" }, undefined, 200_000)).toBe(200_000);
  expect(contextWindowTokens({ mode: "short" }, 64_000, 128_000)).toBe(64_000);
  expect(customContextMaximum()).toBe(2_000_000); // A default is not a hard ceiling.
});

test("custom uses real maxima without inventing a ceiling for an unknown model", () => {
  expect(contextWindowTokens({ mode: "custom", tokens: 240_000 }, 128_000)).toBe(128_000);
  expect(contextWindowTokens({ mode: "custom", tokens: 240_000 })).toBe(240_000);
  expect(customContextMaximum(128_000)).toBe(128_000);
  expect(customContextMaximum(4_000_000)).toBe(2_000_000);
  expect(customContextMaximum()).toBe(2_000_000);
});
