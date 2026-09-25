import { ChevronDown, List } from "lucide-react";
import { useEffect, useId, useLayoutEffect, useRef, useState, type FocusEvent, type RefObject } from "react";
import { useTranslation } from "react-i18next";

type Section = { id: string; label: string; target: HTMLElement };

export function HelpContents({ documentRef, language }: {
  documentRef: RefObject<HTMLElement | null>;
  language: string;
}) {
  const { t } = useTranslation();
  const [sections, setSections] = useState<Section[]>([]);
  const [activeId, setActiveId] = useState("");
  const [open, setOpen] = useState(false);
  const navigationRef = useRef<HTMLElement>(null);
  const listId = useId();

  useLayoutEffect(() => {
    const article = documentRef.current;
    const scroller = article?.closest<HTMLElement>(".relay-content");
    if (!article || !scroller) return;

    // Reuse the guide's own links and headings so translations do not need
    // a second, manually maintained navigation tree.
    const nextSections = Array.from(article.querySelectorAll<HTMLAnchorElement>(".help-source-contents a"))
      .flatMap((link): Section[] => {
        const id = decodeURIComponent(link.hash.slice(1));
        const target = article.querySelector<HTMLElement>(`#${CSS.escape(id)}`);
        return target ? [{ id, label: link.textContent ?? "", target }] : [];
      });
    setSections(nextSections);
    setOpen(false);
    let frame = 0;
    const update = () => {
      frame = 0;
      const firstHeading = nextSections[0]?.target;
      const offset = firstHeading ? Number.parseFloat(getComputedStyle(firstHeading).scrollMarginTop) || 0 : 0;
      const readingTop = scroller.getBoundingClientRect().top + offset + 8;
      const atEnd = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 2;
      let current = nextSections[0];
      for (const section of nextSections) {
        if (!atEnd && section.target.getBoundingClientRect().top > readingTop) break;
        current = section;
      }
      setActiveId(current?.id ?? "");
    };
    const scheduleUpdate = () => {
      if (!frame) frame = requestAnimationFrame(update);
    };
    update();
    scroller.addEventListener("scroll", scheduleUpdate, { passive: true });
    const resizeObserver = new ResizeObserver(scheduleUpdate);
    resizeObserver.observe(article);
    resizeObserver.observe(scroller);
    return () => {
      cancelAnimationFrame(frame);
      scroller.removeEventListener("scroll", scheduleUpdate);
      resizeObserver.disconnect();
    };
  }, [documentRef, language]);

  const closeOnBlur = (event: FocusEvent<HTMLElement>) => {
    if (!event.currentTarget.contains(event.relatedTarget)) setOpen(false);
  };

  useEffect(() => {
    if (!open) return;
    const closeOnPointerDown = (event: PointerEvent) => {
      const target = event.target;
      if (!(target instanceof Node) || !navigationRef.current?.contains(target)) setOpen(false);
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      setOpen(false);
      navigationRef.current?.querySelector<HTMLButtonElement>(".help-contents-toggle")?.focus({ preventScroll: true });
    };
    const closeOnViewportChange = () => setOpen(false);
    document.addEventListener("pointerdown", closeOnPointerDown);
    document.addEventListener("keydown", closeOnEscape, true);
    window.addEventListener("resize", closeOnViewportChange);
    return () => {
      document.removeEventListener("pointerdown", closeOnPointerDown);
      document.removeEventListener("keydown", closeOnEscape, true);
      window.removeEventListener("resize", closeOnViewportChange);
    };
  }, [open]);

  return <nav
    ref={navigationRef}
    className="help-contents"
    aria-label={t("helpCenter.contents")}
    data-open={open}
    onBlur={closeOnBlur}
  >
    <div className="help-contents-label"><List aria-hidden /><span>{t("helpCenter.contents")}</span></div>
    <button
      className="help-contents-toggle"
      type="button"
      aria-label={t("helpCenter.contents")}
      aria-expanded={open}
      aria-controls={listId}
      onClick={() => setOpen((value) => !value)}
    >
      <List aria-hidden />
      <span><small>{t("helpCenter.contents")}</small><strong>{sections.find((section) => section.id === activeId)?.label}</strong></span>
      <ChevronDown aria-hidden />
    </button>
    <ol id={listId} className="help-contents-list">
      {sections.map((section) => <li key={section.id}>
        <a href={`#${section.id}`} aria-current={activeId === section.id ? "location" : undefined} onClick={() => {
          setOpen(false);
          section.target.focus({ preventScroll: true });
        }}>{section.label}</a>
      </li>)}
    </ol>
  </nav>;
}
