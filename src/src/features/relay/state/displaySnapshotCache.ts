// Display-only copies of redacted management DTOs. Never cache credentials,
// revealed identities, provider bodies, or use these copies as native readiness.
const DATABASE = "relay.displaySnapshots.v1";
type StoredSnapshot<T> = { key: string; value: T; savedAt: number };
let database: Promise<IDBDatabase | null> | undefined;

function openDatabase() {
  return database ??= new Promise<IDBDatabase | null>((resolve) => {
    try {
      const request = indexedDB.open(DATABASE, 1);
      request.onupgradeneeded = () => {
        for (const name of ["runtime", "usage", "proxies"]) {
          const store = request.result.createObjectStore(name, { keyPath: "key" });
          store.createIndex("savedAt", "savedAt");
        }
      };
      request.onsuccess = () => resolve(request.result);
      request.onerror = request.onblocked = () => resolve(null);
    } catch { resolve(null); }
  });
}

/** Bounded last-good views; IndexedDB keeps JSON serialization off the UI path. */
export function createDisplaySnapshotCache<T>(
  storeName: "runtime" | "usage" | "proxies",
  limit: number,
  valid: (value: unknown) => value is T,
  memoryLimit = limit,
) {
  const entries = new Map<string, T>();
  const pending = new Map<string, StoredSnapshot<T>>();
  const restoringKeys = new Map<string, Promise<void>>();
  let hydration: Promise<void> | undefined;
  let generation = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;

  const trim = () => {
    while (entries.size > memoryLimit) entries.delete(entries.keys().next().value!);
  };
  const restore = async (key?: string) => {
    const requestedGeneration = generation;
    const db = await openDatabase();
    if (!db || requestedGeneration !== generation) return;
    await new Promise<void>((resolve) => {
      try {
        const store = db.transaction(storeName).objectStore(storeName);
        const request = key === undefined ? store.index("savedAt").getAll() : store.get(key);
        request.onerror = () => resolve();
        request.onsuccess = () => {
          if (generation === requestedGeneration) {
            const fresh = key === undefined ? new Map(entries) : [];
            const records = key === undefined
              ? (request.result as StoredSnapshot<unknown>[]).slice(-memoryLimit)
              : [request.result as StoredSnapshot<unknown> | undefined];
            for (const record of records) {
              try {
                if (record && typeof record.key === "string"
                  && (key === undefined || record.key === key && !entries.has(key))
                  && valid(record.value)) entries.set(record.key, record.value);
              } catch { /* Invalid saved data must not prevent native reads. */ }
            }
            // Whole-store hydration must keep newer native values last for LRU.
            for (const [freshKey, value] of fresh) { entries.delete(freshKey); entries.set(freshKey, value); }
            trim();
          }
          resolve();
        };
      } catch { resolve(); }
    });
  };
  const flush = async () => {
    clearTimeout(timer);
    timer = undefined;
    if (!pending.size) return;
    const records = [...pending.values()];
    pending.clear();
    const requestedGeneration = generation;
    const db = await openDatabase();
    if (!db || requestedGeneration !== generation) return;
    try {
      const store = db.transaction(storeName, "readwrite").objectStore(storeName);
      records.forEach((record) => store.put(record));
      const keys = store.index("savedAt").getAllKeys();
      keys.onsuccess = () => keys.result.slice(0, Math.max(0, keys.result.length - limit)).forEach((key) => store.delete(key));
    } catch { /* A display cache must never block native data or rendering. */ }
  };
  if (typeof window !== "undefined") {
    window.addEventListener("pagehide", () => { void flush(); });
    document.addEventListener("visibilitychange", () => {
      if (document.visibilityState !== "visible") void flush();
    });
  }

  return {
    peek: (key: string) => entries.get(key) ?? null,
    restoreKey(key: string) {
      if (entries.has(key)) return Promise.resolve();
      const existing = restoringKeys.get(key);
      if (existing) return existing;
      const restoring = restore(key);
      restoringKeys.set(key, restoring);
      void restoring.finally(() => {
        if (restoringKeys.get(key) === restoring) restoringKeys.delete(key);
      });
      return restoring;
    },
    restore: () => hydration ??= restore(),
    remember(key: string, value: T) {
      entries.delete(key);
      entries.set(key, value);
      trim();
      pending.set(key, { key, value, savedAt: Date.now() });
      while (pending.size > limit) pending.delete(pending.keys().next().value!);
      timer ??= setTimeout(() => { void flush(); }, 250);
    },
    async clear() {
      generation += 1;
      restoringKeys.clear();
      entries.clear();
      pending.clear();
      clearTimeout(timer);
      timer = undefined;
      const db = await openDatabase();
      if (!db) return;
      await new Promise<void>((resolve) => {
        try {
          const transaction = db.transaction(storeName, "readwrite");
          transaction.objectStore(storeName).clear();
          transaction.oncomplete = transaction.onerror = transaction.onabort = () => resolve();
        } catch { resolve(); }
      });
    },
  };
}
