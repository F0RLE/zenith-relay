// Disposable public-client issuer. Keys, codes and synthetic subjects live only in this process.
import { createServer } from "node:http";
import { createHash, generateKeyPairSync, randomUUID, sign } from "node:crypto";

const enc = v => Buffer.from(JSON.stringify(v)).toString("base64url");
export async function startIssuer(issuer, redirect, client) {
  const url = new URL(issuer);
  if (url.protocol !== "http:" || url.hostname !== "127.0.0.1" || url.origin !== issuer) throw new Error("Expected loopback issuer");
  const { publicKey, privateKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
  const otherKey = generateKeyPairSync("rsa", { modulusLength: 2048 }).privateKey;
  const codes = new Map();
  const counts = { exchanges: 0, accepted: 0 };
  const server = createServer(async (req, res) => {
    const request = new URL(req.url, issuer);
    const send = (status, v) => { res.writeHead(status, { "Content-Type": "application/json" }); res.end(JSON.stringify(v)); };
    if (request.pathname === "/keys") return send(200, { keys: [{ ...publicKey.export({ format: "jwk" }), kid: "one", use: "sig", alg: "RS256" }] });
    if (req.method === "GET" && request.pathname === "/authorize") {
      const q = request.searchParams, identity = /(?:^|;\s*)test_subject=([a-z-]+)/.exec(req.headers.cookie ?? "")?.[1];
      if (!identity || q.get("client_id") !== client || q.get("redirect_uri") !== redirect
        || q.get("scope") !== "openid" || q.get("response_type") !== "code" || q.get("code_challenge_method") !== "S256"
        || !q.get("state") || !q.get("nonce") || !q.get("code_challenge")) return send(400, {});
      const code = randomUUID();
      codes.set(code, { identity, challenge: q.get("code_challenge"), nonce: q.get("nonce") });
      const cb = new URL(redirect); cb.search = new URLSearchParams({ state: q.get("state"), code, iss: issuer });
      res.writeHead(303, { Location: cb.href, "Referrer-Policy": "no-referrer", "Cache-Control": "no-store" }); return res.end();
    }
    if (req.method !== "POST" || request.pathname !== "/token") return send(404, {});
    counts.exchanges++;
    let raw = ""; for await (const part of req) raw += part;
    const f = new URLSearchParams(raw), code = f.get("code"), auth = codes.get(code);
    codes.delete(code);
    if (!auth || f.get("grant_type") !== "authorization_code" || f.get("client_id") !== client
      || f.get("redirect_uri") !== redirect || !f.get("code_verifier")
      || createHash("sha256").update(f.get("code_verifier")).digest("base64url") !== auth.challenge) return send(400, {});
    counts.accepted++;
    const now = Math.floor(Date.now()/1000), kind = auth.identity;
    const claims = { iss: kind === "wrong-issuer" ? issuer + "/other" : issuer, aud: kind === "wrong-audience" ? "other" : client,
      sub: kind, email: "same@example.invalid", iat: now, exp: now + (kind === "expired" ? -30 : 60),
      ...(kind === "not-yet-valid" ? { nbf: now + 45 } : kind === "bob" ? { nbf: now - 1 } : {}),
      nonce: kind === "wrong-nonce" ? randomUUID() : auth.nonce, azp: kind === "wrong-party" ? "other" : client };
    const unsigned = `${enc({ typ: "JWT", alg: "RS256", kid: "one" })}.${enc(claims)}`;
    const sig = sign("RSA-SHA256", Buffer.from(unsigned), kind === "wrong-signature" ? otherKey : privateKey).toString("base64url");
    send(200, { token_type: "Bearer", access_token: randomUUID(), id_token: `${unsigned}.${sig}` });
  });
  await new Promise((resolve, reject) => server.once("error", reject).listen(Number(url.port), "127.0.0.1", resolve));
  return { counts, close: async () => { server.closeAllConnections(); await new Promise(resolve => server.close(resolve)); } };
}
