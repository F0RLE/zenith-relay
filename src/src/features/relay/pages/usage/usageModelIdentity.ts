const CODEX_ALIAS_PREFIX = "zenith/";
const DESKTOP_ALIAS_PREFIXES = [
  "claude-sonnet-5-relay-", "claude-fable-5-relay-",
  "claude-zenith-", "claude-fable-5-zenith-",
];

function isDesktopAlias(model: string): boolean {
  const normalized = model.toLowerCase();
  return DESKTOP_ALIAS_PREFIXES.some((prefix) => normalized.startsWith(prefix));
}

function decodeCodexAlias(model: string): string | null {
  if (!model.toLowerCase().startsWith(CODEX_ALIAS_PREFIX)) return null;
  const encoded = model.slice(CODEX_ALIAS_PREFIX.length);
  if (!encoded || !/^[A-Za-z0-9_-]+$/.test(encoded)) return null;
  try {
    const padded = encoded.replace(/-/g, "+").replace(/_/g, "/")
      + "=".repeat((4 - encoded.length % 4) % 4);
    const bytes = Uint8Array.from(globalThis.atob(padded), (character) => character.charCodeAt(0));
    const decoded = new TextDecoder().decode(bytes);
    return decoded && decoded.length <= 256 && !/[\u0000-\u001f]/.test(decoded) ? decoded : null;
  } catch {
    return null;
  }
}

/** Opaque Desktop aliases need observed routing evidence, not guessed names. */
export function modelIdForDisplay(model: string | null | undefined, resolvedModel?: string | null): string | null {
  if (!model) return null;
  if (isDesktopAlias(model)) {
    return resolvedModel && !isDesktopAlias(resolvedModel) ? modelIdForDisplay(resolvedModel) : null;
  }
  return decodeCodexAlias(model) ?? model;
}

export function formatModelId(model: string | null | undefined, resolvedModel?: string | null): string {
  return modelIdForDisplay(model, resolvedModel) ?? "—";
}
