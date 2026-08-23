import { lazy, Suspense } from "react";
import { Loader2 } from "lucide-react";

const MarkdownDescription = lazy(() => import("../../components/MarkdownDescription"));

export function MarkdownPreview({ content }: { content: string }) {
  return <Suspense fallback={<div className="markdown-loading" role="status"><Loader2 className="spin" aria-hidden /></div>}><MarkdownDescription content={content} /></Suspense>;
}
