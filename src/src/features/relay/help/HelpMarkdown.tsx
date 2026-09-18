import type { ReactNode } from "react";
import type { Components } from "react-markdown";

export const helpMarkdownComponents: Components = {
  h1: ({ children }) => <h1 id={headingId(children)}>{children}</h1>,
  h2: ({ children }) => <h2 id={headingId(children)} tabIndex={-1}>{children}</h2>,
  h3: ({ children }) => <h3 id={headingId(children)} tabIndex={-1}>{children}</h3>,
  p: ({ children, node }) => {
    const isContents = node?.children.some((child) => child.type === "element" && child.tagName === "a")
      && node.children.every((child) => child.type === "text"
        ? child.value.replaceAll("|", "").trim() === ""
        : child.type === "element" && child.tagName === "a" && String(child.properties.href).startsWith("#"));
    return isContents ? <div className="help-source-contents" hidden>{children}</div> : <p>{children}</p>;
  },
  table: ({ children }) => <div className="help-table-wrap"><table>{children}</table></div>,
};

function headingId(children: ReactNode) {
  return String(children).toLocaleLowerCase().trim().replace(/[^\p{L}\p{N}]+/gu, "-").replace(/^-|-$/g, "");
}
