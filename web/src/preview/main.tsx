import { createRoot } from "react-dom/client";
import "../styles.css";

if (import.meta.env.DEV) {
  const { PreviewApp } = await import("./PreviewApp");
  createRoot(document.getElementById("root")!).render(<PreviewApp />);
}
