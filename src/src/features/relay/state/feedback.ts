export type FeedbackError = {
  code: string;
  message: string;
  reason?: string;
  model?: string;
  source?: string;
  route?: string;
  requestId?: string;
  status?: number;
  retryable?: boolean;
};

const MAX_FEEDBACK_CODE_LENGTH = 120;
const MAX_FEEDBACK_MESSAGE_LENGTH = 600;
const MAX_FEEDBACK_FIELD_LENGTH = 160;
const SAFE_CODE = /^[a-z0-9][a-z0-9_.:-]{0,119}$/i;

export function sanitizeFeedbackError(error: unknown, fallbackCode = "general", fallbackMessage = ""): FeedbackError {
  const envelope = isRecord(error) && isRecord(error.error) ? error.error : error;
  const payload = isRecord(envelope) && isRecord(envelope.diagnostic)
    ? { ...envelope, ...envelope.diagnostic }
    : envelope;
  const rawCode = isRecord(payload) && typeof payload.code === "string"
    ? payload.code
    : fallbackCode;
  const code = normalizeCode(rawCode, fallbackCode);
  const rawMessage = isRecord(payload) && typeof payload.message === "string"
    ? payload.message
    : error instanceof Error
      ? error.message
      : typeof error === "string"
        ? error
        : fallbackMessage;
  const message = redactFeedbackText(rawMessage || fallbackMessage || code);
  const diagnostic: FeedbackError = { code, message: message || code };
  const reason = diagnosticText(payload, ["reason", "stage", "category"]);
  const model = diagnosticText(payload, ["model", "modelId", "resolvedModel"]);
  const source = diagnosticText(payload, ["source", "sourceId", "provider"]);
  const route = diagnosticText(payload, ["route", "endpoint", "wireApi"]);
  const requestId = diagnosticText(payload, ["requestId", "request_id"]);
  const status = diagnosticStatus(payload);
  const retryable = isRecord(payload) && typeof payload.retryable === "boolean"
    ? payload.retryable
    : undefined;

  if (reason) diagnostic.reason = reason;
  if (model) diagnostic.model = model;
  if (source) diagnostic.source = source;
  if (route) diagnostic.route = route;
  if (requestId) diagnostic.requestId = requestId;
  if (status !== undefined) diagnostic.status = status;
  if (retryable !== undefined) diagnostic.retryable = retryable;
  return diagnostic;
}

function normalizeCode(value: string, fallback: string) {
  const candidate = value.trim().slice(0, MAX_FEEDBACK_CODE_LENGTH);
  if (SAFE_CODE.test(candidate)) return candidate;
  const safeFallback = fallback.trim().slice(0, MAX_FEEDBACK_CODE_LENGTH);
  return SAFE_CODE.test(safeFallback) ? safeFallback : "general";
}

function diagnosticText(value: unknown, fields: string[]) {
  if (!isRecord(value)) return undefined;
  for (const field of fields) {
    if (typeof value[field] !== "string") continue;
    const text = redactFeedbackText(value[field]).slice(0, MAX_FEEDBACK_FIELD_LENGTH);
    if (text) return text;
  }
  return undefined;
}

function diagnosticStatus(value: unknown) {
  if (!isRecord(value)) return undefined;
  const candidate = value.status ?? value.statusCode ?? value.httpStatus;
  if (typeof candidate !== "number" || !Number.isInteger(candidate) || candidate < 100 || candidate > 599) {
    return undefined;
  }
  return candidate;
}

// Error messages can contain provider echoes, so keep only a short, redacted diagnostic.
export function redactFeedbackText(value: string) {
  return value
    .replace(/[\r\n\t]+/g, " ")
    .replace(/\s{2,}/g, " ")
    .trim()
    .slice(0, MAX_FEEDBACK_MESSAGE_LENGTH)
    .replace(/Bearer\s+[^\s,;]+/gi, "Bearer [redacted]")
    .replace(/\b(?:eyJ[A-Za-z0-9_-]*\.){2}[A-Za-z0-9_-]+\b/g, "[redacted JWT]")
    .replace(/((?:api[_-]?key|x[_-]?api[_-]?key|access[_-]?token|refresh[_-]?token|id[_-]?token|authorization|password|client[_-]?secret|secret|token|set[_-]?cookie|cookie|session(?:[_-]?id)?|csrf(?:[_-]?token)?)\s*[:=]\s*)("[^"]*"|'[^']*'|[^\s,;]+)/gi, "$1[redacted]")
    .replace(/([?&](?:api[_-]?key|x[_-]?api[_-]?key|access[_-]?token|refresh[_-]?token|id[_-]?token|authorization|password|client[_-]?secret|secret|token|set[_-]?cookie|cookie|session(?:[_-]?id)?|csrf(?:[_-]?token)?)=)[^&\s]*/gi, "$1[redacted]")
    .replace(/\b(?:sk|pk|rk|znt|zrs|ghp|github_pat|xox[baprs]-|at-)[A-Za-z0-9_-]{8,}\b/gi, "[redacted]");
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}
