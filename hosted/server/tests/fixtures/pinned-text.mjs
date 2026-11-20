import { readFixture } from "../../../test-support/files.mjs";
import { createServer } from "node:https";

const [port, cert, key] = process.argv.slice(2);
const counts = { catalog: 0, responses: 0, unexpected: 0 };
const payload = (res, value) => res.writeHead(200, { "content-type": "application/json" })
  .end(JSON.stringify(value));

if (!/^\d+$/.test(port) || +port < 1 || +port > 65535) throw new Error("Invalid fixture port");
createServer({ cert: readFixture(cert), key: readFixture(key) }, async (req, res) => {
  if (req.headers.host === `fixture.test:${port}`) {
    if (req.method === "GET" && req.url === "/counts") return payload(res, counts);
    if (req.headers.authorization === "Bearer fixture-provider") {
      if (req.method === "GET" && req.url === "/v1/models") {
        counts.catalog++;
        return payload(res, { object: "list", data: [{ id: "fixture-text", object: "model" }] });
      }
      if (req.method === "POST" && req.url === "/v1/responses") {
        counts.responses++;
        const chunks = [];
        for await (const chunk of req) chunks.push(chunk);
        const body = JSON.parse(Buffer.concat(chunks).toString());
        if (body.input === "redirect") return res.writeHead(307, { location: "/trap" }).end();
        if (body.model === "fixture-text" && body.store === false) {
          return payload(res, {
            object: "response", id: "resp_https_fixture", model: body.model, status: "completed",
            output: [{ type: "message", role: "assistant", content: [{ type: "output_text", text: "OK" }] }],
          });
        }
      }
    }
  }
  counts.unexpected++;
  res.writeHead(400).end();
}).listen(+port, "127.0.0.1");
