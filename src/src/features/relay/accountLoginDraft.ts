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

export function mergeAccountLoginDraft(existingDraft: AccountLoginDraft, draft: AccountLoginDraft): AccountLoginDraft {
  return {
    email: draft.email || existingDraft.email,
    phone: draft.phone || existingDraft.phone,
    password: draft.password || existingDraft.password,
    totpSecret: draft.totpSecret || existingDraft.totpSecret,
  };
}

export type LoginNoteEdits = Record<keyof AccountLoginDraft, boolean>;

export function editedLoginNotes(saved: AccountLoginDraft, latestDraft: AccountLoginDraft, dirty: LoginNoteEdits): AccountLoginDraft | null {
  const updatedDraft = {
    email: dirty.email ? latestDraft.email : saved.email,
    phone: dirty.phone ? latestDraft.phone : saved.phone,
    password: dirty.password ? latestDraft.password : saved.password,
    totpSecret: dirty.totpSecret ? latestDraft.totpSecret : saved.totpSecret,
  };
  if (updatedDraft.email === saved.email && updatedDraft.phone === saved.phone && updatedDraft.password === saved.password && updatedDraft.totpSecret === saved.totpSecret) return null;
  return updatedDraft;
}
