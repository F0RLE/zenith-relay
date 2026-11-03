import type { LocalUsagePage } from "../api/types";
import { createDisplaySnapshotCache } from "./displaySnapshotCache";

export const usagePreview = createDisplaySnapshotCache("usage", 8, (value): value is LocalUsagePage => {
  if (!value || typeof value !== "object") return false;
  const report = value as LocalUsagePage;
  return (report.hasMore === undefined || typeof report.hasMore === "boolean")
    && Number.isFinite(report.total) && Number.isFinite(report.page) && Number.isFinite(report.totalPages)
    && Number.isFinite(report.totals?.requests) && report.totals?.apiEquivalent != null
    && Array.isArray(report.events) && Array.isArray(report.models) && Array.isArray(report.poolMembers)
    && report.events.every((event) => event && typeof event.requestId === "string" && Number.isFinite(Date.parse(event.createdAt)))
    && [...report.models, ...report.poolMembers].every((group) => group && typeof group.key === "string" && group.totals?.apiEquivalent != null);
}, 1);

export function rememberUsagePreview(key: string, report: LocalUsagePage) {
  // Request details can re-read native diagnostics. Never duplicate provider
  // error messages in the display cache, even when native redaction is enabled.
  const events = report.events.some((event) => event.upstreamError != null)
    ? report.events.map((event) => event.upstreamError == null ? event : { ...event, upstreamError: null })
    : report.events;
  usagePreview.remember(key, events === report.events ? report : { ...report, events });
}
