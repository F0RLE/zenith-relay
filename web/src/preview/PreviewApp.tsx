import { HostedApp } from "../features/app/HostedApp";
import { previewSnapshot } from "./snapshot";
import { usePreviewCatalog } from "./usePreviewCatalog";

export function PreviewApp() {
  const catalogController = usePreviewCatalog({ rules: previewSnapshot.owner.modelRules ?? {}, modelDisplayOrder: previewSnapshot.owner.modelDisplayOrder ?? [] });
  return <HostedApp snapshot={previewSnapshot} catalogController={catalogController} />;
}
