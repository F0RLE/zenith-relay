import { expect, test } from "bun:test";
import { parseDevices, validDeviceApproval, validDeviceLabel } from "../../src/api/devices";

const origin = "https://relay.example";
const device = "00000000-0000-4000-8000-000000000002";
const row = { id: device, label: "Laptop", state: "active", scope: "connections.read",
  revision: "2", expires_at_s: 10, last_used_at_s: null };
const list = { origin, installation_id: "00000000-0000-4000-8000-000000000001", items: [row] };

test("device metadata is bound to the installation origin and the one supported scope", () => {
  expect(parseDevices(list, origin).items[0]?.id).toBe(device);
  expect(() => parseDevices(list, "https://another.example")).toThrow();
  for (const changes of [{scope:"admin"}, {id:""}, {revision:"02"}, {state:"pending"},
    {state:"revoked",revision:"1"}, {state:"expired",revision:"3"}, {label:"\nlabel"}, {expires_at_s:0},
    {state:"pending",revision:"1",last_used_at_s:5}]) {
    expect(() => parseDevices({...list, items:[{...row,...changes}]}, origin)).toThrow();
  }
  expect(() => parseDevices({...list,items:[row,row]},origin)).toThrow();
});

test("approval accepts only the exact device's review digest, and labels are bounded in bytes", () => {
  const digest = "ab".repeat(32);
  expect(validDeviceApproval(device,`review1.${device}.${digest}`)).toBe(true);
  for (const code of [`review1.${list.installation_id}.${digest}`,`rhd1.${device}.${digest}`,
    `review1.${device}.${digest.toUpperCase()}`,`review1.${device}.${digest}x`]) {
    expect(validDeviceApproval(device,code)).toBe(false);
  }
  expect(validDeviceLabel("я".repeat(32))).toBe(true);
  for (const label of ["я".repeat(33)," x","", "a\u0000b"]) expect(validDeviceLabel(label)).toBe(false);
});
