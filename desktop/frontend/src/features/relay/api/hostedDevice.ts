import { invoke } from "./commands";
import type {
  HostedCopyReview, HostedCopyState, HostedDevicePage, HostedDeviceRead, HostedDeviceReview,
  HostedHours, HostedOverview, HostedPoolChange, HostedPoolReview, HostedReadScope, HostedUsage, HostedUsageFilter,
} from "./types/hostedDevice";

export const getPairing = () => invoke<HostedDeviceReview | null>("get_hosted_device");
export const preparePairing = (invitation: string) => invoke<HostedDeviceReview>("prepare_hosted_device", { invitation });
export const forgetPairing = (invitation: string) => invoke<void>("forget_hosted_device", { invitation });
export const readConnections = (invitation: string, after?: string) =>
  invoke<HostedDevicePage>("read_hosted_connections", { invitation, after: after ?? null });
export const preparePoolChange = (invitation: string, change: HostedPoolChange) =>
  invoke<HostedPoolReview>("prepare_hosted_pool_change", { invitation, change });
export const readAccess = (invitation: string) =>
  invoke<HostedDeviceRead<HostedReadScope[]> | null>("read_hosted_access", { invitation });
export const readOverview = (invitation: string, hours: HostedHours) =>
  invoke<HostedDeviceRead<HostedOverview>>("read_hosted_overview", { invitation, hours });
export const readUsage = (invitation: string, filter: HostedUsageFilter) =>
  invoke<HostedDeviceRead<HostedUsage>>("read_hosted_usage", { invitation, filter });
export const openDemo = () => invoke<void>("open_hosted_preview");
export const listCopySources = (invitation: string) =>
  invoke<{ id: string; label: string }[]>("list_hosted_copy_sources", { invitation });
export const getCopyReview = (invitation: string) =>
  invoke<HostedCopyReview | null>("get_hosted_copy", { invitation });
export const prepareCopy = (invitation: string, sourceId: string) =>
  invoke<HostedCopyReview>("prepare_hosted_copy", { invitation, sourceId });
export const checkCopy = (invitation: string, operationId: string) =>
  invoke<HostedCopyState>("hosted_copy_status", { invitation, operationId });
export const confirmCopy = (invitation: string, operationId: string) =>
  invoke<HostedCopyState>("confirm_hosted_copy", { invitation, operationId });
export const clearCopy = (invitation: string, operationId: string) =>
  invoke<void>("clear_hosted_copy", { invitation, operationId });
