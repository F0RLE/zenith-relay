import { useEffect, useRef } from "react";
import type { ReactNode } from "react";
import { X } from "lucide-react";

export function Dialog({ title, onClose, children, className = "" }: { title: string; onClose: () => void; children: ReactNode; className?: string }) {
  const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const dialog = ref.current!;
    const origin = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const overflow = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    dialog.showModal();
    dialog.querySelector<HTMLElement>("[data-initial-focus]")?.focus({ preventScroll: true });
    return () => {
      dialog.close();
      document.body.style.overflow = overflow;
      if (origin?.isConnected) origin.focus({ preventScroll: true });
    };
  }, []);

  return <dialog ref={ref} className={`request-dialog ${className}`.trim()} aria-labelledby="request-dialog-title"
    onCancel={event => { event.preventDefault(); onClose(); }}>
    <header className="dialog-header"><h2 id="request-dialog-title">{title}</h2>
      <button type="button" className="icon-button" aria-label="Закрыть" onClick={onClose}><X aria-hidden="true" /></button>
    </header>
    <div className="dialog-body">{children}</div>
  </dialog>;
}
