import { describe, expect, test } from "bun:test";
import { editedLoginNotes, mergeAccountLoginDraft } from "../src/features/relay/accountLoginDraft";

describe("account login draft", () => {
  test("keeps the signed-in email when the note only adds a password", () => {
    expect(mergeAccountLoginDraft({
      email: "person@example.test",
      phone: "",
      password: "",
      totpSecret: "",
    }, {
      email: "",
      phone: "950000000",
      password: "synthetic-password",
      totpSecret: "GEZDGNBVGY3TQOJQ",
    })).toEqual({
      email: "person@example.test",
      phone: "950000000",
      password: "synthetic-password",
      totpSecret: "GEZDGNBVGY3TQOJQ",
    });
  });

  test("closing notes does not replace stored values that were not edited", () => {
    const saved = {
      email: "person@example.test",
      phone: "950000000",
      password: "synthetic-password",
      totpSecret: "GEZDGNBVGY3TQOJQ",
    };
    expect(editedLoginNotes(saved, {
      email: "person@example.test",
      phone: "",
      password: "",
      totpSecret: "",
    }, {
      email: false,
      phone: false,
      password: false,
      totpSecret: false,
    })).toBeNull();
    expect(editedLoginNotes(saved, {
      email: "other@example.test",
      phone: "",
      password: "",
      totpSecret: "",
    }, {
      email: true,
      phone: false,
      password: false,
      totpSecret: false,
    })).toEqual({
      ...saved,
      email: "other@example.test",
    });
  });
});
