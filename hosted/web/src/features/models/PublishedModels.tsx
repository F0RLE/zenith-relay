import { useEffect, useState } from "react";
import { readListings, type Listing } from "../../api/catalog";
import { ControlError, observeControl } from "../../api/control";
import { ModelsPage } from "./ModelsPage";
import type { CustomerModel } from "./model";

export function PublishedModels() {
  const [rows, setRows] = useState<Listing[]>();
  const [failed, setFailed] = useState(false);
  const [reload, setReload] = useState(0);
  useEffect(() => observeControl(readListings,
    v => { setRows(v); setFailed(false); }, err => {
      if (err instanceof ControlError && err.status === 401) location.replace("/login");
      else { setRows(undefined); setFailed(true); }
    }), [reload]);
  const models = rows?.map((row): CustomerModel => ({
    id: row.model, name: row.model, protocols: ["Responses"], description: "", revision: row.revision,
    ...(row.publisher !== null && { publisher: row.publisher }),
    rates: { input: row.rates.input, output: row.rates.output, cacheRead: row.rates.cached },
  }));
  if (failed) return <ModelsPage state={{ kind: "unavailable", retry: () => {
    setFailed(false);
    setReload(n => n + 1);
  } }} />;
  return models ? <ModelsPage models={models} /> : <ModelsPage state={{ kind: "loading" }} />;
}
