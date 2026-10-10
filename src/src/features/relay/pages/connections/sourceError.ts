import { sanitizeFeedbackError } from "../../state/feedback";

/** Keep provider diagnostics redacted and show a short reason on the card. */
export function sourceErrorDetails(message: string | null | undefined) {
  if (!message?.trim()) return null;
  const error = sanitizeFeedbackError(message, "source_check_failed");
  const statusMatch = error.message.match(/\bHTTP\s+([45]\d{2})\b/i);
  const status = statusMatch ? Number(statusMatch[1]) : undefined;
  let summary = "Source check failed";
  if (status === 400) summary = "API request rejected";
  else if (status === 401) summary = "API authentication failed";
  else if (status === 403) summary = "API access denied";
  else if (status === 429) summary = "API rate limit reached";
  else if (status === 404) summary = "API endpoint not found";
  else if (status && status >= 500) summary = "API temporarily unavailable";
  else if (/timeout|timed out/i.test(error.message)) summary = "API request timed out";
  else if (/transport|network|connect/i.test(error.message)) summary = "API connection failed";
  else if (/model.*discovery|discovery.*model/i.test(error.message)) summary = "Model discovery failed";
  if (status) {
    error.status = status;
    summary += ` · HTTP ${status}`;
  }
  return { error, summary };
}
