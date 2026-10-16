import type { TFunction } from "i18next";
import { sanitizeFeedbackError } from "../../state/feedback";

/** Keep provider diagnostics redacted and show a short reason on the card. */
export function sourceErrorDetails(message: string | null | undefined, t: TFunction) {
  if (!message?.trim()) return null;
  const error = sanitizeFeedbackError(message, "source_check_failed");
  const statusMatch = error.message.match(/\bHTTP\s+([45]\d{2})\b/i);
  const status = statusMatch ? Number(statusMatch[1]) : undefined;
  let reason = "failed";
  if (status === 400) reason = "request";
  else if (status === 401) reason = "authentication";
  else if (status === 403) reason = "access";
  else if (status === 429) reason = "rateLimit";
  else if (status === 404) reason = "endpoint";
  else if (status && status >= 500) reason = "unavailable";
  else if (/timeout|timed out/i.test(error.message)) reason = "timeout";
  else if (/transport|network|connect/i.test(error.message)) reason = "connection";
  else if (/model.*discovery|discovery.*model/i.test(error.message)) reason = "models";
  let summary = t(`sources.errors.${reason}`);
  if (status) {
    error.status = status;
    summary += ` · HTTP ${status}`;
  }
  return { error, summary };
}
