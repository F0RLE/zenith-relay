import { listen } from "@tauri-apps/api/event";
import { invoke } from "./commands";
import type { OAuthEditAction, OAuthEditMenuEvent, OAuthEditResult } from "./types/oauthEdit";

export const oauthEditCommands = {
  onMenu: (callback: (event: OAuthEditMenuEvent) => void) => listen<OAuthEditMenuEvent>("relay-oauth-edit-menu", (event) => callback(event.payload)),
  edit: (loginId: string, requestId: string, action: OAuthEditAction, value?: string) => invoke<OAuthEditResult>("edit_codex_oauth_view", { loginId, requestId, action, ...(value !== undefined ? { value } : {}) }),
};
