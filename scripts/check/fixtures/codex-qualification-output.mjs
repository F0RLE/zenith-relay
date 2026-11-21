import { isDeepStrictEqual } from "node:util";

// SIWC may complete with output: [] after sending every item as an indexed done event.
// Use these snapshots for the qualification callback only; never rewrite the client's SSE.
export class CodexQualificationOutput {
  #slots = new Map();
  #responseId;
  #unindexedDelta = false;

  observe(event) {
    for (const id of [event.response_id, event.response?.id]) {
      if (id === undefined) continue;
      if (typeof id !== "string" || !id || (this.#responseId !== undefined && this.#responseId !== id)) {
        throw new Error("Conflicting stream response identity");
      }
      this.#responseId = id;
    }
    const added = event.type === "response.output_item.added", done = event.type === "response.output_item.done";
    if (!added && !done && !event.type?.endsWith(".delta")) return;
    const index = event.output_index;
    if (index === undefined && !added && !done) { this.#unindexedDelta = true; return; }
    if (!Number.isSafeInteger(index) || index < 0 || index >= 1024) throw new Error("Unproven output index");
    const slot = this.#slots.get(index) ?? {};
    if (slot.done && !done) throw new Error("Output continued after its completed snapshot");
    if (added || done) {
      const item = event.item;
      if (!item || typeof item.type !== "string" || (slot.type !== undefined && slot.type !== item.type)
        || (slot.id !== undefined && item.id !== slot.id) || (slot.done && !isDeepStrictEqual(slot.done, item))) {
        throw new Error("Conflicting output item");
      }
      slot.type = item.type;
      slot.id ??= item.id;
      if (done) slot.done = item;
    }
    this.#slots.set(index, slot);
  }

  finish(response) {
    if (response?.status !== "completed") return response;
    if (response.output !== undefined && !Array.isArray(response.output)) throw new Error("Invalid terminal output");
    const full = response.output?.length > 0;
    if (!full && this.#unindexedDelta) throw new Error("Unproven output index");
    for (const [index, slot] of this.#slots) {
      const item = full ? response.output[index] : slot.done;
      if (!item || (slot.type !== undefined && slot.type !== item.type) || (slot.id !== undefined && slot.id !== item.id)
        || (full && slot.done && !isDeepStrictEqual(item, slot.done))) throw new Error("Unproven terminal output item");
    }
    if (full || !this.#slots.size) return response;
    const output = Array.from({ length: this.#slots.size }, (_, i) => this.#slots.get(i)?.done);
    if (output.some(item => !item)) throw new Error("Incomplete terminal output sequence");
    return { ...response, output };
  }
}
