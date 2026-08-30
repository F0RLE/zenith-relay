/** Returns a compact source label without rejecting a user-entered URL. */
export function sourceHost(value: string) {
  try {
    return new URL(value).host;
  } catch {
    return value;
  }
}
