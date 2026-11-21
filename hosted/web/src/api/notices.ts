import { control, record, uuidText } from "./control";
import { confirmOwnerAction } from "./ownerSession";

export type Notice = { id: string; title: string; message: string };
type PublishedNotice = Notice & { publishedAt: number; read: boolean };
export type OwnerNotice = Notice & { revision: number } & (
  { publishedAt: null; recipients: null } | { publishedAt: number; recipients: number });
export type NoticeCommand = { operation_id: string; expected_revision: number; command:
  | { action: "save"; title: string; message: string }
  | { action: "publish" | "discard" } };

export function noticeFields(title: string, message: string) {
  const fields = { title: title.trim(), message: message.trim() };
  if (fields.title.length < 1 || fields.title.length > 120 || fields.message.length < 1 || fields.message.length > 4000
    || /[\u0000-\u001f\u007f-\u009f]/u.test(fields.title) || /[\u0000-\u0008\u000b-\u001f\u007f-\u009f]/u.test(fields.message)) return;
  return fields;
}

function item(v: unknown) {
  if (!record(v) || typeof v.id !== "string" || !uuidText.test(v.id)
    || typeof v.title !== "string" || typeof v.message !== "string" || !noticeFields(v.title, v.message))
    throw new Error("Invalid notification");
  return { id: v.id, title: v.title, message: v.message };
}

function timestamp(v: unknown): v is number {
  return typeof v === "number" && Number.isSafeInteger(v) && v > 0 && v <= 8.64e15;
}

function items(data: Record<string, unknown>, max: number): unknown[] {
  if (!Array.isArray(data.items) || data.items.length > max) throw new Error("Invalid notification list");
  return data.items;
}

export function parseOwnerNotices(data: Record<string, unknown>): OwnerNotice[] {
  return items(data, 48).map(v => {
    if (!record(v) || typeof v.revision !== "number" || !Number.isInteger(v.revision) || v.revision < 1 || v.revision > 2147483647)
      throw new Error("Invalid notification revision");
    const draft = { ...item(v), revision: v.revision };
    if (v.published_at_ms === null && v.recipient_count === null) return { ...draft, publishedAt: null, recipients: null };
    if (timestamp(v.published_at_ms) && typeof v.recipient_count === "number"
      && Number.isInteger(v.recipient_count) && v.recipient_count >= 1 && v.recipient_count <= 16)
      return { ...draft, publishedAt: v.published_at_ms, recipients: v.recipient_count };
    throw new Error("Invalid publication");
  });
}

export async function ownerNotices(signal: AbortSignal) {
  return parseOwnerNotices(await control("admin/notifications", { signal }));
}

export async function changeNotice(id: string, csrf: string, command: NoticeCommand) {
  const path = "admin/notifications/" + id;
  if (command.command.action === "publish") {
    return confirmOwnerAction(path, csrf, command, "admin/notifications/confirmations");
  }
  const data = await control(path, {
    method: "POST", headers: { "Content-Type": "application/json", "X-Relay-CSRF": csrf },
    body: JSON.stringify(command),
  });
  if (data.applied !== true || data.operation_id !== command.operation_id) throw new Error("Notification not acknowledged");
}

export function parseCustomerNotices(data: Record<string, unknown>): PublishedNotice[] {
  return items(data, 32).map(v => {
    if (!record(v) || !timestamp(v.published_at_ms) || typeof v.read !== "boolean") throw new Error("Invalid inbox notification");
    return { ...item(v), publishedAt: v.published_at_ms, read: v.read };
  });
}

export async function customerNotices(signal: AbortSignal) {
  return parseCustomerNotices(await control("notifications", { signal }));
}

export async function markNoticesRead(csrf: string, ids: string[]) {
  const data = await control("notifications/read", {
    method: "POST", headers: { "Content-Type": "application/json", "X-Relay-CSRF": csrf },
    body: JSON.stringify({ ids }),
  });
  if (data.applied !== true) throw new Error("Reading notifications not acknowledged");
}
