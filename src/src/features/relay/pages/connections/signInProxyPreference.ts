const STORAGE_KEY = "relay.signInProxyId";
const PROXY_ID = /^[A-Za-z0-9_-]{1,80}$/;

export function rememberedSignInProxyId() {
  try {
    const storedProxyId = localStorage.getItem(STORAGE_KEY)?.trim() ?? "";
    return PROXY_ID.test(storedProxyId) ? storedProxyId : null;
  } catch {
    return null;
  }
}

export function rememberSignInProxyId(proxyId: string) {
  if (!PROXY_ID.test(proxyId)) return;
  try {
    localStorage.setItem(STORAGE_KEY, proxyId);
  } catch {
    // The last choice is only a shortcut. Sign-in still works from the list.
  }
}

export function isHttpProxyEndpoint(endpoint: string) {
  return endpoint.startsWith("http://");
}
