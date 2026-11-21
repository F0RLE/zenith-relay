export function hostedErrorKey(cause: unknown) {
  switch (cause) {
    case "copy_review_pending": return "hostedCopy.pending";
    case "device_not_authorized": return "hosted.denied";
    case "device_storage_unavailable": return "hosted.storage";
    case "invalid_device_invitation": return "hosted.invalid";
    case "invalid_device_filter": return "hosted.filterExpired";
    case "desktop_pool_changed": return "hostedPool.changed";
    default: return "hosted.unavailable";
  }
}
