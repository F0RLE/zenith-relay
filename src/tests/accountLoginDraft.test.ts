import { describe, expect, test } from "bun:test";
import { editedLoginNotes, forgetAccountLoginDraft, getAccountLoginDraft, mergeAccountLoginDraft, rememberAccountLoginDraft, takeAccountLoginDraft } from "../src/features/relay/accountLoginDraft";

describe("account login draft", () => {
  test("reopening notes reads a draft without consuming it", () => {
    const draft = { email: "person@example.test", phone: "", password: "synthetic-password", totpSecret: "" };
    rememberAccountLoginDraft("synthetic-reopen", draft);
    expect(getAccountLoginDraft("synthetic-reopen")).toEqual(draft);
    expect(getAccountLoginDraft("synthetic-reopen")).toEqual(draft);
    expect(takeAccountLoginDraft("synthetic-reopen")).toEqual(draft);
    expect(getAccountLoginDraft("synthetic-reopen")).toBeUndefined();
  });

  test("drafts stay isolated by attempt and cancellation removes only its draft", () => {
    const draft = { email: "", phone: "", password: "synthetic-password", totpSecret: "" };
    rememberAccountLoginDraft("synthetic-first", draft);
    rememberAccountLoginDraft("synthetic-second", { ...draft, password: "synthetic-second-password" });
    forgetAccountLoginDraft("synthetic-first");
    expect(getAccountLoginDraft("synthetic-first")).toBeUndefined();
    expect(getAccountLoginDraft("synthetic-second")?.password).toBe("synthetic-second-password");
    rememberAccountLoginDraft("synthetic-second", { email: "", phone: "", password: "", totpSecret: "" });
    expect(getAccountLoginDraft("synthetic-second")).toBeUndefined();
  });

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
