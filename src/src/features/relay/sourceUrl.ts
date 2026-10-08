/** Returns a compact source label without rejecting a user-entered URL. */
export function sourceHost(sourceText: string) {
  try {
    return new URL(sourceText).host;
  } catch {
    return sourceText;
  }
}

/** Returns an explicitly configured URL port without rejecting partial state. */
export function sourcePort(sourceText: string) {
  try {
    return new URL(sourceText).port;
  } catch {
    return "";
  }
}
