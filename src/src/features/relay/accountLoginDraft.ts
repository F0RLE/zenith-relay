export type AccountLoginDraft = {
  email: string;
  phone: string;
  password: string;
  totpSecret: string;
};

const drafts = new Map<string, AccountLoginDraft>();

export function rememberAccountLoginDraft(loginId: string, draft: AccountLoginDraft) {
  if (!draft.email && !draft.phone && !draft.password && !draft.totpSecret) {
    drafts.delete(loginId);
    return;
  }
  drafts.set(loginId, draft);
}

export function takeAccountLoginDraft(loginId: string) {
  const draft = drafts.get(loginId);
  drafts.delete(loginId);
  return draft;
}

export function forgetAccountLoginDraft(loginId: string) {
  drafts.delete(loginId);
}

export function mergeAccountLoginDraft(current: AccountLoginDraft, draft: AccountLoginDraft): AccountLoginDraft {
  return {
    email: draft.email || current.email,
    phone: draft.phone || current.phone,
    password: draft.password || current.password,
    totpSecret: draft.totpSecret || current.totpSecret,
  };
}

export type LoginNoteEdits = Record<keyof AccountLoginDraft, boolean>;

export function editedLoginNotes(saved: AccountLoginDraft, current: AccountLoginDraft, dirty: LoginNoteEdits): AccountLoginDraft | null {
  const next = {
    email: dirty.email ? current.email : saved.email,
    phone: dirty.phone ? current.phone : saved.phone,
    password: dirty.password ? current.password : saved.password,
    totpSecret: dirty.totpSecret ? current.totpSecret : saved.totpSecret,
  };
  if (next.email === saved.email && next.phone === saved.phone && next.password === saved.password && next.totpSecret === saved.totpSecret) return null;
  return next;
}
