import { gfmFromMarkdown } from "mdast-util-gfm";
import { gfm } from "micromark-extension-gfm";
import type {} from "remark-parse";
import type { Plugin } from "unified";

/** Same GFM reader as remark-gfm, without the unused Markdown writer. */
export const remarkGfmReadOnly: Plugin = function () {
  const data = this.data();
  (data.micromarkExtensions ??= []).push(gfm());
  (data.fromMarkdownExtensions ??= []).push(gfmFromMarkdown());
};
