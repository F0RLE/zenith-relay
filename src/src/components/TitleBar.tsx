import { useEffect, useState } from "react";
import { Minus, Square, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { closeWindow, minimizeWindow, toggleMaximizeWindow, watchWindowFullscreen, type Platform } from "../platform/desktop";

type TitleBarProps = {
  platform: Platform | "unknown";
};

export function TitleBar({ platform }: TitleBarProps) {
  const { t } = useTranslation();
  const macos = platform === "macos";
  const [fullscreen, setFullscreen] = useState(false);
  useEffect(() => {
    if (!macos) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void watchWindowFullscreen((value) => {
      if (!disposed) setFullscreen(value);
    }).then((stop) => {
      if (disposed) stop();
      else unlisten = stop;
    }).catch(() => undefined);
    return () => { disposed = true; unlisten?.(); };
  }, [macos]);
  const stylePlatform = platform === "unknown" ? "linux" : platform;
  const controls = (
    <div className={`window-controls window-controls-${stylePlatform}`}>
      <button className="minimize" type="button" onClick={() => minimizeWindow()} aria-label={t("window.minimize")}>
        <Minus aria-hidden />
      </button>
      <button className="maximize" type="button" onClick={() => toggleMaximizeWindow()} aria-label={t("window.maximize")}>
        <Square aria-hidden />
      </button>
      <button className="close" type="button" onClick={() => closeWindow()} aria-label={t("window.close")}>
        <X aria-hidden />
      </button>
    </div>
  );

  return (
    <header className={`titlebar titlebar-${stylePlatform}${fullscreen ? " titlebar-fullscreen" : ""}`}>
      {macos ? <div className="titlebar-native-controls" aria-hidden /> : null}
      <div className="titlebar-drag" data-tauri-drag-region>
        <img className="titlebar-logo" src="/icons/zenith-sword.png" alt="" data-tauri-drag-region />
        <strong data-tauri-drag-region>{t("app.label")}</strong>
      </div>
      {!macos ? controls : null}
    </header>
  );
}
