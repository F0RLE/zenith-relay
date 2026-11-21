import { createRoot } from "react-dom/client";
import { LocaleProvider } from "../i18n/LocaleProvider";
import "../styles.css";

if (import.meta.env.DEV) {
  const { PreviewApp } = await import("./PreviewApp");
  createRoot(document.getElementById("root")!).render(<LocaleProvider><PreviewApp /></LocaleProvider>);
}
