import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";

const allowedElements = [
  "a", "blockquote", "br", "code", "del", "em", "h1", "h2", "h3", "h4", "h5", "h6", "hr", "img",
  "li", "ol", "p", "pre", "strong", "table", "tbody", "td", "th", "thead", "tr", "ul",
];
const components: Components = {
  a: ({ href, children }) => <span className="markdown-safe-link" title={href}>{children}</span>,
  img: () => null,
};

export default function MarkdownDescription({ content }: { content: string }) {
  return <div className="markdown-description"><ReactMarkdown skipHtml allowedElements={allowedElements} components={components} remarkPlugins={[remarkGfm]}>{content}</ReactMarkdown></div>;
}
