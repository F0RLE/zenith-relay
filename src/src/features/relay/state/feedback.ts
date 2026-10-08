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
const SENSITIVE_VALUE_FIELD = "(?:api[_-]?key|x[_-]?api[_-]?key|access[_-]?token|refresh[_-]?token|id[_-]?token|authorization|password|2fa|totp(?:[_-]?secret)?|otp(?:[_-]?secret)?|phone|client[_-]?secret|secret|token|set[_-]?cookie|cookie|session(?:[_-]?id)?|csrf(?:[_-]?token)?|account(?:[_-]?(?:id|email|identity|name|label))?|user(?:[_-]?(?:id|email|identity|name))?|email|identity)";
const SENSITIVE_VALUE = new RegExp(`(["']?${SENSITIVE_VALUE_FIELD}["']?\\s*[:=]\\s*)("(?:\\\\.|[^"\\\\])*"|'(?:\\\\.|[^'\\\\])*'|[^\\s,;}]+)`, "gi");
const SENSITIVE_QUERY_VALUE = new RegExp(`([?&]${SENSITIVE_VALUE_FIELD}=)[^&\\s]*`, "gi");
const URL_CREDENTIALS = /([a-z][a-z\d+.-]*:\/\/)[^\s/@:]+:[^\s/@]+@/gi;

export function sanitizeFeedbackError(error: unknown, fallbackCode = "general", fallbackMessage = ""): FeedbackError {
  const envelope = isRecord(error) && isRecord(error["error"]) ? error["error"] : error;
  const diagnosticPayload = isRecord(envelope) && isRecord(envelope["diagnostic"])
    ? { ...envelope, ...envelope["diagnostic"] }
    : envelope;
  const rawCode = isRecord(diagnosticPayload) && typeof diagnosticPayload["code"] === "string"
    ? diagnosticPayload["code"]
    : fallbackCode;
  const code = normalizeCode(rawCode, fallbackCode);
  const rawMessage = isRecord(diagnosticPayload) && typeof diagnosticPayload["message"] === "string"
    ? diagnosticPayload["message"]
    : error instanceof Error
      ? error.message
      : typeof error === "string"
        ? error
        : fallbackMessage;
  const message = redactFeedbackText(rawMessage || fallbackMessage || code);
  const diagnostic: FeedbackError = { code, message: message || code };
  const reason = diagnosticText(diagnosticPayload, ["reason", "stage", "category"]);
  const model = diagnosticText(diagnosticPayload, ["model", "modelId", "resolvedModel"]);
  const source = diagnosticText(diagnosticPayload, ["source", "sourceId", "provider"]);
  const route = diagnosticText(diagnosticPayload, ["route", "endpoint", "wireApi"]);
  const requestId = diagnosticText(diagnosticPayload, ["requestId", "request_id"]);
  const status = diagnosticStatus(diagnosticPayload);
  const retryable = isRecord(diagnosticPayload) && typeof diagnosticPayload["retryable"] === "boolean"
    ? diagnosticPayload["retryable"]
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

function normalizeCode(codeText: string, fallback: string) {
  const candidate = codeText.trim().slice(0, MAX_FEEDBACK_CODE_LENGTH);
  if (SAFE_CODE.test(candidate)) return candidate;
  const safeFallback = fallback.trim().slice(0, MAX_FEEDBACK_CODE_LENGTH);
  return SAFE_CODE.test(safeFallback) ? safeFallback : "general";
}

function diagnosticText(diagnosticPayload: unknown, fields: string[]) {
  if (!isRecord(diagnosticPayload)) return undefined;
  for (const field of fields) {
    const fieldValue = diagnosticPayload[field];
    if (typeof fieldValue !== "string") continue;
    const text = redactFeedbackText(fieldValue).slice(0, MAX_FEEDBACK_FIELD_LENGTH);
    if (text) return text;
  }
  return undefined;
}

function diagnosticStatus(diagnosticPayload: unknown) {
  if (!isRecord(diagnosticPayload)) return undefined;
  const candidate = diagnosticPayload["status"] ?? diagnosticPayload["statusCode"] ?? diagnosticPayload["httpStatus"];
  if (typeof candidate !== "number" || !Number.isInteger(candidate) || candidate < 100 || candidate > 599) {
    return undefined;
  }
  return candidate;
}

// Error messages can contain provider echoes, so keep only a short, redacted diagnostic.
export function redactFeedbackText(feedbackText: string) {
  return feedbackText
    .replace(/[\r\n\t]+/g, " ")
    .replace(/\s{2,}/g, " ")
    .trim()
    .replace(/Bearer\s+[^\s,;]+/gi, "Bearer [redacted]")
    .replace(/\b(?:eyJ[A-Za-z0-9_-]*\.){2}[A-Za-z0-9_-]+\b/g, "[redacted JWT]")
    .replace(URL_CREDENTIALS, "$1[redacted]@")
    .replace(/\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b/gi, "[redacted email]")
    .replace(SENSITIVE_VALUE, "$1[redacted]")
    .replace(SENSITIVE_QUERY_VALUE, "$1[redacted]")
    .replace(/\b(?:account|user)[_-][A-Z0-9][A-Z0-9_-]{3,}\b/gi, "[redacted identity]")
    .replace(/\b(?:sk|pk|rk|znt|zrs|ghp|github_pat|xox[baprs]-|at-)[A-Za-z0-9_-]{8,}\b/gi, "[redacted]")
    .slice(0, MAX_FEEDBACK_MESSAGE_LENGTH);
}

function isRecord(recordValue: unknown): recordValue is Record<string, unknown> {
  return typeof recordValue === "object" && recordValue !== null;
}
