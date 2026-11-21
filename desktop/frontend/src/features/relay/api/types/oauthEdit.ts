export type OAuthEditAction = "read" | "cut" | "paste" | "selectAll" | "dismiss";
export type OAuthEditResult = { ok: boolean; text?: string };
export type OAuthEditMenuEvent = {
  loginId: string;
  requestId: string;
  x: number;
  y: number;
  canCopy: boolean;
  canCut: boolean;
  canPaste: boolean;
  canSelectAll: boolean;
};
