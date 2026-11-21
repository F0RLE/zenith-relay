import { useRef, useState } from "react";
import { changeSourceServing, checkSourceServing, type SavedSource, type Serving, type ServingChange } from "../../api/connections";
import { ControlError } from "../../api/control";
import { OwnerPasskeyError } from "../../api/ownerSession";
import { Dialog } from "../../components/Dialog";
import { useLocale } from "../../i18n/LocaleProvider";
import type { MessageKey } from "../../i18n/types";

export type ServingAction = ServingChange["action"];
type Reviewed = Extract<ServingAction, "park" | "trash" | "restore" | "remove">;
type ServingTarget = Pick<SavedSource, "id" | "label" | "serving">;
type Target = { source: ServingTarget; command: ServingChange };
type Pending = Target & { phase: "sending" | "lookup" | "retry" };
export const servingLabels = {
  pause: "connections.pause", resume: "connections.resume", park: "connections.removeFromPool",
  trash: "connections.moveToTrash", restore: "connections.restoreOutsidePool", remove: "sourceServing.remove",
} as const satisfies Record<ServingAction, MessageKey>;
const consequences = {
  park: "sourceServing.parkEffect", trash: "sourceServing.trashEffect", restore: "sourceServing.restoreEffect",
  remove: "sourceServing.removeEffect",
} as const satisfies Record<Reviewed, MessageKey>;
const confirmLabels = {
  park: "sourceServing.confirmPark", trash: "sourceServing.confirmTrash", restore: "sourceServing.confirmRestore",
  remove: "sourceServing.confirmRemove",
} as const satisfies Record<Reviewed, MessageKey>;

export function SourceServingStatus({ view, member = view.in_pool }: { view: Serving; member?: boolean }) {
  const { t } = useLocale();
  const membership = view.state === "trashed" ? "connections.inTrash" :
    member ? "connections.inPool" : "connections.outsidePool";
  return <span className="source-state" role="status">
    {t(membership)}
    {(view.draining || view.state === "paused") && <>
      <span aria-hidden="true"> · </span>{t(view.draining ? "sourceServing.draining" : "connections.paused")}
    </>}
  </span>;
}

export function useSourceServing({ sources, csrf, stale, refresh, focusTab }: {
  sources: readonly ServingTarget[]; csrf: string; stale: boolean; refresh: () => void; focusTab: () => void;
}) {
  const { t } = useLocale();
  const [pending, setPending] = useState<Pending>();
  const [settling, setSettling] = useState<{ id: string; revision: string }>();
  const [message, setMessage] = useState<{ key: MessageKey; source: ServingTarget }>();
  const [review, setReview] = useState<Target & { action: Reviewed }>();
  const flight = useRef(false);
  const recoveryButton = useRef<HTMLButtonElement>(null);

  function waiting(source: ServingTarget) {
    return settling?.id === source.id && settling.revision === source.serving?.revision;
  }

  function select(source: ServingTarget, action: ServingAction) {
    if (stale || pending || flight.current || waiting(source) || !source.serving ||
      (source.serving.draining && action !== "park" && action !== "trash")) return;
    const target = { source, command: { action, expected_revision: source.serving.revision, operation_id: crypto.randomUUID() } };
    setMessage(undefined);
    if (action === "pause" || action === "resume") void act(target);
    else setReview({ ...target, action });
  }

  async function act(target: Target, phase?: "lookup" | "retry") {
    if (flight.current) return;
    const { source, command } = target;
    const focusedRecovery = document.activeElement === recoveryButton.current;
    flight.current = true;
    setReview(undefined);
    setPending({ ...target, phase: "sending" });
    setMessage(undefined);
    let retry: Pending | undefined;
    let key: MessageKey | undefined;
    try {
      if (phase === "lookup") await checkSourceServing(source.id, csrf, command);
      else await changeSourceServing(source.id, csrf, command);
      setSettling({ id: source.id, revision: command.expected_revision });
      refresh();
    } catch (error) {
      if (error instanceof ControlError && error.status === 401) location.replace("/admin/login");
      else if (phase === "lookup" && error instanceof ControlError && error.code === "fresh_passkey_required") {
        retry = { ...target, phase: "retry" };
        key = "catalog.retry";
      } else if (error instanceof ControlError && (error.status === 404 || error.code === "source_serving_changed"
        || error.code === "source_draining")) {
        key = "sourceServing.changed";
        refresh();
      } else if (error instanceof OwnerPasskeyError) {
        if (phase) retry = { ...target, phase };
        key = "access.passkeyFailed";
      } else if (error instanceof ControlError && [400, 403, 429].includes(error.status)) {
        if (phase) retry = { ...target, phase };
        key = "catalog.retry";
      } else {
        retry = { ...target, phase: "lookup" };
        key = "sourceServing.noReply";
      }
    } finally {
      flight.current = false;
      setPending(retry);
      setMessage(key ? { key, source } : undefined);
      if (focusedRecovery) requestAnimationFrame(() => {
        if (document.activeElement !== document.body) return;
        if (recoveryButton.current) recoveryButton.current.focus();
        else focusTab();
      });
    }
  }

  const resultSource = pending?.source ?? message?.source;
  const result = resultSource && <div className="source-serving">
    <strong>{resultSource.label ?? resultSource.id}</strong>
    {message && <small role="alert">{t(message.key)}</small>}
    {pending?.phase === "sending" && <small role="status">{t("unavailable.connecting")}</small>}
    {pending && pending.phase !== "sending" && <button ref={recoveryButton} type="button" className="key-button key-secondary"
      onClick={() => void act(pending, pending.phase === "lookup" ? "lookup" : "retry")}>
      {t(pending.phase === "lookup" ? "copy.checkResult" : servingLabels[pending.command.action])}
    </button>}
    {!pending && <button type="button" className="key-button key-secondary" onClick={() => setMessage(undefined)}>
      {t("common.close")}
    </button>}
  </div>;
  const current = review && sources.find(source => source.id === review.source.id);
  const changed = !!review && (!current?.serving || current.serving.revision !== review.command.expected_revision ||
    (current.serving.draining && review.action !== "park" && review.action !== "trash"));
  const cancel = () => setReview(undefined);
  const dialog = review && <Dialog className="owner-source-dialog" title={t(servingLabels[review.action])} onClose={cancel}>
    <strong className="saved-source-name">{review.source.label ?? review.source.id}</strong>
    <p id="source-serving-effect">{t(consequences[review.action])}</p>
    {changed && <p role="alert">{t("sourceServing.changed")}</p>}
    {!changed && stale && <p role="alert">{t("unavailable.server")}</p>}
    <div className="button-row">
      <button data-initial-focus type="button" className="key-button key-secondary" onClick={cancel}>{t("common.cancel")}</button>
      <button type="button" className={`key-button ${review.action === "remove" ? "key-secondary danger-button" : "key-primary"}`}
        aria-describedby="source-serving-effect" disabled={stale || changed}
        onClick={() => void act(review)}>{t(confirmLabels[review.action])}</button>
    </div>
  </Dialog>;
  return { select, waiting, blocked: stale || !!pending, result, dialog };
}
