import { useEffect, useRef, useState } from "react";
import { ControlError } from "../../api/control";
import { OwnerPasskeyError } from "../../api/ownerSession";
import { changePoolRotation, isPoolMode, readPoolRotation, type PoolRotation, type PoolRotationChange } from "../../api/poolRotation";
import { Dialog } from "../../components/Dialog";
import { useLocale } from "../../i18n/LocaleProvider";
import type { MessageKey } from "../../i18n/types";
import { OwnerPoolRotation } from "./OwnerPoolRotation";
import type { PoolRoutingPolicy } from "./poolRotation";

function fromSnapshot(snapshot: PoolRotation): PoolRoutingPolicy {
  return {
    version: 3, mode: snapshot.mode,
    members: snapshot.members.map(row => ({ id: row.source_id, kind: "source", weight: 1, maxConcurrency: 0 })),
  };
}

export function LivePoolRotation({ csrf, close, saved }: {
  csrf: string; close: () => void; saved: () => void;
}) {
  const { t } = useLocale();
  const [snapshot, setSnapshot] = useState<PoolRotation>();
  const [draft, setDraft] = useState<PoolRoutingPolicy>();
  const [error, setError] = useState<MessageKey>();
  const [readFailed, setReadFailed] = useState(false);
  const [reload, setReload] = useState(0);
  const [busy, setBusy] = useState(false);
  const [pending, setPending] = useState<PoolRotationChange>();
  const saving = useRef(false);

  useEffect(() => {
    const abort = new AbortController();
    void readPoolRotation(AbortSignal.any([abort.signal, AbortSignal.timeout(5000)])).then(value => {
      if (abort.signal.aborted) return;
      setSnapshot(value);
      setDraft(fromSnapshot(value));
    }, e => {
      if (abort.signal.aborted) return;
      if (e instanceof ControlError && e.status === 401) location.replace("/admin/login");
      setReadFailed(true);
      setError(e instanceof ControlError && (e.status === 404 || e.code === "pool_rotation_capacity")
        ? "pool.rotation.unavailable" : "unavailable.server");
    });
    return () => abort.abort();
  }, [reload]);

  function reread(note?: MessageKey) {
    setSnapshot(undefined);
    setDraft(undefined);
    setPending(undefined);
    setError(note);
    setReadFailed(false);
    setReload(n => n + 1);
  }

  const orderChanged = snapshot && draft?.mode === "failover"
    && draft.members.some((row, index) => row.id !== snapshot.members[index]?.source_id);
  const dirty = !!(snapshot && draft && (draft.mode !== snapshot.mode || orderChanged));
  async function save() {
    if (!snapshot || !draft || !isPoolMode(draft.mode) || !dirty || saving.current) return;
    const command = pending ?? {
      operation_id: crypto.randomUUID(), scope_id: snapshot.scope_id, expected_revision: snapshot.revision,
      mode: draft.mode, member_order: draft.mode === "failover"
        ? draft.members.map(row => row.id) : snapshot.members.map(row => row.source_id),
    };
    saving.current = true;
    setBusy(true);
    setError(undefined);
    try {
      await changePoolRotation(csrf, command);
      saved();
    } catch (e) {
      if (e instanceof ControlError && e.status === 401) location.replace("/admin/login");
      else if (e instanceof ControlError && (e.status === 409 || e.status === 400)) reread("pool.rotation.changed");
      else if (e instanceof ControlError && e.code === "passkey_rate_limited") {
        setPending(undefined);
        setError("ownerPasskeys.rate");
      } else if (e instanceof OwnerPasskeyError) {
        setPending(undefined);
        setError("access.passkeyFailed");
      } else {
        setPending(command);
        setError("catalog.retry");
      }
    } finally {
      saving.current = false;
      setBusy(false);
    }
  }

  return <Dialog className="owner-rotation-dialog live-rotation-dialog" title={t("pool.routingSettings")}
    onClose={close} dismissible={!busy}>
    <div className="live-rotation-pane">
      {snapshot && draft ? <fieldset className="live-rotation-form" disabled={busy || !!pending}>
        <legend className="sr-only">{t("pool.rotation.strategy")}</legend>
        <OwnerPoolRotation embedded restricted policy={draft} connections={snapshot.members.map(row => ({
          id: row.source_id, name: row.label, kind: "api", paused: row.paused,
        }))} onChange={next => { setDraft(next); setError(undefined); }} />
      </fieldset> : !readFailed && <p className="live-rotation-note" role="status">{t("unavailable.connecting")}</p>}
    </div>
    {error && <p className="live-rotation-note" role="alert">{t(error)}</p>}
    {busy && <span className="sr-only" role="status">{t("unavailable.connecting")}</span>}
    <footer className="live-rotation-footer">
      <button type="button" className="relay-button" disabled={busy} onClick={close}>{t("common.cancel")}</button>
      <button type="button" className="relay-button primary" disabled={!dirty && !readFailed} aria-disabled={busy || undefined}
        onClick={() => { if (readFailed) reread(); else void save(); }}>{t(readFailed || pending ? "common.retry" : "common.save")}</button>
    </footer>
  </Dialog>;
}
