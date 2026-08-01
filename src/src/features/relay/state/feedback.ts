export type FeedbackError = {
  code: string;
  message: string;
};

const MAX_FEEDBACK_CODE_LENGTH = 120;
const MAX_FEEDBACK_MESSAGE_LENGTH = 600;
const SAFE_CODE = /^[a-z0-9][a-z0-9_.:-]{0,119}$/i;

export function sanitizeFeedbackError(error: unknown, fallbackCode = "general", fallbackMessage = ""): FeedbackError {
  const rawCode = isRecord(error) && typeof error.code === "string"
    ? error.code
    : fallbackCode;
  const code = normalizeCode(rawCode, fallbackCode);
  const rawMessage = isRecord(error) && typeof error.message === "string"
    ? error.message
    : error instanceof Error
      ? error.message
      : typeof error === "string"
        ? error
        : fallbackMessage;
  const message = redactFeedbackText(rawMessage || fallbackMessage || code);
  return { code, message: message || code };
}

function normalizeCode(value: string, fallback: string) {
  const candidate = value.trim().slice(0, MAX_FEEDBACK_CODE_LENGTH);
  if (SAFE_CODE.test(candidate)) return candidate;
  const safeFallback = fallback.trim().slice(0, MAX_FEEDBACK_CODE_LENGTH);
  return SAFE_CODE.test(safeFallback) ? safeFallback : "general";
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
