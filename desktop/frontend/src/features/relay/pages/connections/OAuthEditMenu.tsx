import { CheckCheck, ClipboardPaste, Copy, Scissors } from "lucide-react";
import { useEffect, useRef, useState, type RefObject } from "react";
import { useTranslation } from "react-i18next";
import { oauthEditCommands } from "../../api/oauthEdit";
import type { OAuthEditMenuEvent } from "../../api/types/oauthEdit";
import { ContextMenu } from "../../components/ContextMenu";
import { copyText } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";

export function OAuthEditMenu({ loginId, active, slot, onReady }: { loginId: string; active: boolean; slot: RefObject<HTMLDivElement | null>; onReady: (loginId: string | null) => void }) {
  const { t } = useTranslation();
  const { perform } = useRelayState();
  const [menu, setMenu] = useState<OAuthEditMenuEvent | null>(null);
  const current = useRef<OAuthEditMenuEvent | null>(null);
  const running = useRef(false);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    if (!active) return;
    let mounted = true;
    let unlisten: (() => void) | undefined;
    void perform("oauth-edit-listen", async () => {
      const stop = await oauthEditCommands.onMenu((event) => {
        if (!mounted || event.loginId !== loginId || !slot.current) return;
        const rect = slot.current.getBoundingClientRect();
        current.current = event;
        setMenu({ ...event, x: rect.left + Math.max(0, Math.min(event.x, rect.width)), y: rect.top + Math.max(0, Math.min(event.y, rect.height)) });
      });
      if (mounted) {
        unlisten = stop;
        onReady(loginId);
      }
      else stop();
    }, undefined, { skipRefresh: true, uiLock: false });
    return () => {
      mounted = false;
      unlisten?.();
      onReady(null);
      const previous = current.current;
      current.current = null;
      setMenu(null);
      if (previous) void oauthEditCommands.edit(previous.loginId, previous.requestId, "dismiss").catch(() => undefined);
    };
  }, [active, loginId, onReady, perform, slot]);

  const close = () => {
    if (running.current) return;
    const previous = current.current;
    current.current = null;
    setMenu(null);
    if (previous) void oauthEditCommands.edit(previous.loginId, previous.requestId, "dismiss").catch(() => undefined);
  };
  const edit = async (action: "copy" | "cut" | "paste" | "selectAll") => {
    const target = current.current;
    if (!target || running.current) return;
    running.current = true;
    setBusy(true);
    await perform("oauth-edit", async () => {
      if (action === "copy" || action === "cut") {
        const result = await oauthEditCommands.edit(loginId, target.requestId, "read");
        if (!result.ok || result.text == null) throw new Error(t("common.copyFailed"));
        await copyText(result.text);
        await oauthEditCommands.edit(loginId, target.requestId, action === "copy" ? "dismiss" : "cut");
      } else {
        const value = action === "paste" ? await navigator.clipboard.readText() : undefined;
        await oauthEditCommands.edit(loginId, target.requestId, action, value);
      }
      if (current.current?.requestId === target.requestId) {
        current.current = null;
        setMenu(null);
      }
    }, undefined, { skipRefresh: true, uiLock: false });
    running.current = false;
    setBusy(false);
  };
  if (!active || !menu) return null;
  return <ContextMenu position={menu} onClose={close}>
    {menu.canPaste ? <button className="relay-popover-item" type="button" role="menuitem" disabled={busy || !menu.canCut} onClick={() => void edit("cut")}><Scissors aria-hidden /><span>{t("common.cut")}</span></button> : null}
    <button className="relay-popover-item" type="button" role="menuitem" disabled={busy || !menu.canCopy} onClick={() => void edit("copy")}><Copy aria-hidden /><span>{t("common.copy")}</span></button>
    {menu.canPaste ? <button className="relay-popover-item" type="button" role="menuitem" disabled={busy} onClick={() => void edit("paste")}><ClipboardPaste aria-hidden /><span>{t("common.paste")}</span></button> : null}
    {menu.canSelectAll ? <button className="relay-popover-item context-menu-select-all" type="button" role="menuitem" disabled={busy} onClick={() => void edit("selectAll")}><CheckCheck aria-hidden /><span>{t("common.selectAllText")}</span></button> : null}
  </ContextMenu>;
}
