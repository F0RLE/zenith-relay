import { Minus, Square, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import {
  closeWindow,
  minimizeWindow,
  toggleFullscreenWindow,
  toggleMaximizeWindow,
  type Platform,
} from "../platform/desktop";

type TitleBarProps = {
  platform: Platform | "unknown";
};

export function TitleBar({ platform }: TitleBarProps) {
  const { t } = useTranslation();
  const macos = platform === "macos";
  const stylePlatform = platform === "unknown" ? "linux" : platform;

  const controls = macos ? (
    <div className="window-controls window-controls-macos" onMouseDown={(event) => event.stopPropagation()}>
      <button className="close" type="button" onClick={() => void closeWindow()} aria-label={t("window.close")}>
        <MacCloseIcon />
      </button>
      <button className="minimize" type="button" onClick={() => void minimizeWindow()} aria-label={t("window.minimize")}>
        <MacMinimizeIcon />
      </button>
      <button className="zoom" type="button" onClick={() => void toggleFullscreenWindow()} aria-label={t("window.zoom")}>
        <MacZoomIcon />
      </button>
    </div>
  ) : (
    <div className={`window-controls window-controls-${stylePlatform}`}>
      <button className="minimize" type="button" onClick={() => void minimizeWindow()} aria-label={t("window.minimize")}>
        <Minus aria-hidden />
      </button>
      <button className="maximize" type="button" onClick={() => void toggleMaximizeWindow()} aria-label={t("window.maximize")}>
        <Square aria-hidden />
      </button>
      <button className="close" type="button" onClick={() => void closeWindow()} aria-label={t("window.close")}>
        <X aria-hidden />
      </button>
    </div>
  );

  return (
    <header className={`titlebar titlebar-${stylePlatform}`}>
      {macos ? controls : null}
      <div className="titlebar-drag" data-tauri-drag-region>
        <img className="titlebar-logo" src="/icons/zenith-sword.png" alt="" data-tauri-drag-region />
        <strong data-tauri-drag-region>{t("app.label")}</strong>
      </div>
      {macos ? null : controls}
    </header>
  );
}

function MacCloseIcon() {
  return (
    <svg viewBox="0 0 12 12" aria-hidden="true">
      <path d="M3.1 3.1 8.9 8.9M8.9 3.1 3.1 8.9" />
    </svg>
  );
}

function MacMinimizeIcon() {
  return (
    <svg viewBox="0 0 12 12" aria-hidden="true">
      <path d="M2.4 6h7.2" />
    </svg>
  );
}

function MacZoomIcon() {
  return (
    <svg viewBox="0 0 12 12" aria-hidden="true">
      <path d="M4.7 2.2H2.2v2.5M7.3 9.8h2.5V7.3M2.4 2.4l2.8 2.8M9.6 9.6 6.8 6.8" />
    </svg>
  );
}
