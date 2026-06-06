import { Minus, Square, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { closeWindow, minimizeWindow, Platform, toggleMaximizeWindow } from "../tauri";

type TitleBarProps = {
  platform: Platform;
};

export function TitleBar({ platform }: TitleBarProps) {
  const { t } = useTranslation();
  const macos = platform === "macos";

  const controls = (
    <div className={`window-controls window-controls-${platform}`}>
      <button className="minimize" type="button" onClick={() => minimizeWindow()} aria-label={t("window.minimize")}>
        {macos ? <span aria-hidden /> : <Minus aria-hidden />}
      </button>
      <button className="maximize" type="button" onClick={() => toggleMaximizeWindow()} aria-label={t("window.maximize")}>
        {macos ? <span aria-hidden /> : <Square aria-hidden />}
      </button>
      <button className="close" type="button" onClick={() => closeWindow()} aria-label={t("window.close")}>
        {macos ? <span aria-hidden /> : <X aria-hidden />}
      </button>
    </div>
  );

  return (
    <header className={`titlebar titlebar-${platform}`} data-tauri-drag-region>
      {macos ? controls : null}
      <div className="titlebar-drag" data-tauri-drag-region />
      {!macos ? controls : null}
    </header>
  );
}
