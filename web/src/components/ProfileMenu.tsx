import { useEffect, useRef } from "react";
import { ChevronUp, LogOut } from "lucide-react";

export function ProfileMenu({ onSignOut }: { onSignOut: () => void }) {
  const menu = useRef<HTMLDetailsElement>(null);
  const trigger = useRef<HTMLElement>(null);
  function close(restoreFocus = false) {
    if (menu.current) menu.current.open = false;
    if (restoreFocus) trigger.current?.focus({ preventScroll: true });
  }
  useEffect(() => {
    const outside = (event: PointerEvent) => {
      if (!menu.current?.contains(event.target as Node)) close();
    };
    const escape = (event: KeyboardEvent) => {
      if (menu.current?.open && event.key === "Escape") {
        event.preventDefault(); close(true);
      }
    };
    document.addEventListener("pointerdown", outside, true);
    document.addEventListener("keydown", escape, true);
    return () => {
      document.removeEventListener("pointerdown", outside, true);
      document.removeEventListener("keydown", escape, true);
    };
  }, []);
  return <details ref={menu} className="profile-menu"
    onToggle={event => { if (event.currentTarget.open) event.currentTarget.querySelector("button")?.focus({ preventScroll: true }); }}
    onBlur={event => { if (!event.currentTarget.contains(event.relatedTarget as Node)) close(); }}>
    <summary ref={trigger} role="button" className="profile sidebar-profile" aria-label="Открыть профиль" aria-haspopup="menu"
      onKeyDown={event => {
        if (event.key === "ArrowUp" || event.key === "ArrowDown") {
          event.preventDefault();
          if (menu.current) menu.current.open = true;
        }
      }}><span className="avatar">Д</span><span>Профиль</span><ChevronUp aria-hidden="true" /></summary>
    <div role="menu" aria-label="Профиль">
      <button type="button" role="menuitem" onClick={() => { close(true); onSignOut(); }}
        onKeyDown={event => {
          if (["ArrowUp", "ArrowDown", "Home", "End"].includes(event.key)) event.preventDefault();
          if (event.key === "Tab" && event.shiftKey) { event.preventDefault(); close(true); }
        }}><LogOut aria-hidden="true" />Выйти</button>
    </div>
  </details>;
}
