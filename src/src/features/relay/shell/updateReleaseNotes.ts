export function localizeReleaseNotes(body: string | undefined, language: string): string {
  if (!body?.trim()) return "";
  const markers = [...body.matchAll(/<!--\s*relay-notes:([a-z0-9-]+)\s*-->/gi)];
  if (!markers.length) return body.trim();

  const sections = new Map<string, string>();
  markers.forEach((marker, index) => {
    const markerLocale = marker[1];
    if (!markerLocale) return;
    sections.set(
      markerLocale.toLowerCase(),
      body.slice((marker.index ?? 0) + marker[0].length, markers[index + 1]?.index).trim(),
    );
  });

  const locale = language.toLowerCase();
  const baseLocale = locale.split("-")[0] ?? locale;
  return sections.get(locale) ?? sections.get(baseLocale) ?? sections.get("en") ?? sections.values().next().value ?? "";
}

export function prepareReleaseNotes(body: string | undefined, language: string, version: string): string {
  const notes = localizeReleaseNotes(body, language);
  if (!notes) return "";

  const normalizedVersion = version.replace(/^v/i, "");
  const escapedVersion = normalizedVersion.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const redundantHeading = new RegExp(
    `^#{1,6}\\s+\\[?v?${escapedVersion}\\]?(?:\\s*[-:\u2013\u2014]\\s*\\d{4}-\\d{2}-\\d{2})?\\s*(?:\\r?\\n)+`,
    "i",
  );
  return notes.replace(redundantHeading, "").trim();
}
