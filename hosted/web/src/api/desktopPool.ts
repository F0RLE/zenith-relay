import { control, record, uuidText } from "./control";
import { confirmDesktopAction } from "./ownerSession";

const path = "admin/desktop/pool";
export const validPoolReviewCode = (code: unknown): code is string =>
  typeof code === "string" && uuidText.test(code) && code !== "00000000-0000-0000-0000-000000000000";
const label = (value: unknown, bytes: number): value is string =>
  typeof value === "string" && value.length > 0 && new TextEncoder().encode(value).length <= bytes && !/\p{Cc}/u.test(value);

export interface DesktopPoolPreview {
  operation_id: string;
  action: "include" | "exclude";
  source_id: string;
  source_label: string;
  device_label: string;
  models: string[];
  applied: boolean;
}

export function parseDesktopPoolPreview(value: unknown, code: string): DesktopPoolPreview {
  if (!record(value) || !validPoolReviewCode(value.operation_id) || value.operation_id !== code ||
    !validPoolReviewCode(value.source_id) || !label(value.source_label, 96) || !label(value.device_label, 64) ||
    (value.action !== "include" && value.action !== "exclude") || typeof value.applied !== "boolean" ||
    !Array.isArray(value.models) || value.models.length === 0 || value.models.length > 128 ||
    !value.models.every((model, i, all) => typeof model === "string" && /^[a-zA-Z0-9][a-zA-Z0-9._:/-]{0,127}$/.test(model) &&
      (i === 0 || all[i - 1] < model))) throw new Error("Invalid pool review");
  return {
    operation_id: value.operation_id, source_id: value.source_id, source_label: value.source_label,
    device_label: value.device_label, action: value.action, applied: value.applied, models: value.models,
  };
}

export async function readDesktopPoolReview(code: string) {
  if (!validPoolReviewCode(code)) throw new Error("Invalid pool review code");
  return parseDesktopPoolPreview(await control(`${path}/${code}`), code);
}

export function approveDesktopPoolReview(preview: DesktopPoolPreview, csrf: string) {
  return confirmDesktopAction(path, csrf, { operation_id: preview.operation_id });
}
