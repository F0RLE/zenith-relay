/** Returns a compact source label without rejecting a user-entered URL. */
export function sourceHost(value: string) {
  try {
    return new URL(value).host;
  } catch {
    return value;
  }
}

/** Returns an explicitly configured URL port without rejecting partial state. */
export function sourcePort(value: string) {
  try {
    return new URL(value).port;
  } catch {
    return "";
  }
}
