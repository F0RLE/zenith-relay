import { readFileSync } from "node:fs";
import { request } from "node:http";
import { createServer } from "node:https";

const [port, web, transfers, certPath, keyPath] = process.argv.slice(2);
if (![port, web, transfers].every(p => /^\d+$/.test(p) && +p > 0 && +p <= 65535)
  || new Set([port, web, transfers]).size !== 3) throw new Error("Expected three isolated test ports");

// Fixture edge: forward only to these two local processes. No logs or retries.
createServer({ cert: readFileSync(certPath), key: readFileSync(keyPath) }, (req, res) => {
  if (req.headers.host !== `localhost:${port}` || !req.url?.startsWith("/")) {
    res.writeHead(400).end();
    return;
  }
  const copy = req.method === "GET" && req.url === "/api/hosted/v1/transfer/status"
    || req.method === "POST" && req.url === "/api/hosted/v1/transfer/commit";
  const upstream = request({
    hostname: "127.0.0.1", port: copy ? transfers : web, agent: false,
    method: req.method, path: req.url, headers: req.headers,
  }, reply => {
    res.writeHead(reply.statusCode, reply.headers);
    reply.pipe(res);
    reply.on("error", () => res.destroy());
  });
  upstream.on("error", () => {
    if (res.headersSent) res.destroy();
    else res.writeHead(502, { "cache-control": "no-store" }).end();
  });
  res.on("close", () => { if (!res.writableEnded) upstream.destroy(); });
  req.pipe(upstream);
}).listen(+port, "127.0.0.1");
