import { describe, expect, test } from "bun:test";
import { setVisibleIdsSelected, toggleSelectedId } from "../src/features/relay/state/selectionModel";

describe("identity selection", () => {
  test("toggles an identity without mutating the input or reordering others", () => {
    const selected = Object.freeze(["account-a", "account-b"]);
    expect(toggleSelectedId(selected, "account-a")).toEqual(["account-b"]);
    expect(toggleSelectedId(selected, "account-c")).toEqual(["account-a", "account-b", "account-c"]);
    expect(selected).toEqual(["account-a", "account-b"]);
  });

  test("removes repeated copies of a deselected identity", () => {
    expect(toggleSelectedId(["a", "b", "a"], "a")).toEqual(["b"]);
  });

  test("selecting filtered rows preserves hidden selections and deduplicates", () => {
    expect(setVisibleIdsSelected(["hidden", "a"], ["a", "b", "b"], true))
      .toEqual(["hidden", "a", "b"]);
  });

  test("deselecting filtered rows does not clear hidden selections", () => {
    expect(setVisibleIdsSelected(["hidden", "a", "b"], ["a", "b"], false))
      .toEqual(["hidden"]);
    expect(setVisibleIdsSelected(["hidden"], [], false)).toEqual(["hidden"]);
  });
});
