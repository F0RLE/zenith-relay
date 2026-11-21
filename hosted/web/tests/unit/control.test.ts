import { expect, spyOn, test } from "bun:test";
import { controlResponse } from "../../src/api/control";

test("control requests stay inside the same-origin API namespace", async () => {
  const fetch = spyOn(globalThis, "fetch").mockResolvedValue(new Response("{}"));
  try {
    for (const path of ["", "../admin", "admin/../keys", "admin/%2e%2e/keys", "/keys", "//example.test",
      "https://example.test", "keys#fragment", "keys\\other", "keys\n", "keys//other", "keys/."])
      await expect(controlResponse(path)).rejects.toThrow("Invalid control path");
    expect(fetch).not.toHaveBeenCalled();
    const controller = new AbortController();
    await controlResponse("admin/usage/requests?model=a%2Fb&before=123", {
      mode: "cors", credentials: "include", redirect: "follow", cache: "force-cache", signal: controller.signal,
    });
    expect(fetch).toHaveBeenCalledWith("/api/hosted/v1/admin/usage/requests?model=a%2Fb&before=123", expect.objectContaining({
      mode: "same-origin", credentials: "same-origin", redirect: "error", cache: "no-store",
    }));
    const signal = (fetch.mock.calls[0][1] as RequestInit).signal!;
    expect(signal.aborted).toBe(false);
    controller.abort();
    expect(signal.aborted).toBe(true);
  } finally { fetch.mockRestore(); }
});
