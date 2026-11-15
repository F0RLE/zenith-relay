import type { PaymentOperation } from "../billing/BillingPage";
import type { PaymentMethod } from "../billing/model";
import type { CustomerModel } from "../models/ModelsPage";
import type { RequestRecord } from "../usage/model";
import type { KeyRecord } from "../keys/KeysPage";
import type { OwnerSnapshot } from "../owner/model";

// The API adapter will eventually supply an authorized snapshot. The UI does not
// infer a session or read credentials or the local desktop runtime.
export interface HostedSnapshot {
  asOf: string;
  requests: RequestRecord[];
  models: CustomerModel[];
  /** Development fixtures only; never set for an authorized retail catalog. */
  modelPricingIllustrative?: boolean;
  balance: bigint;
  reserved: bigint;
  payments: PaymentOperation[];
  paymentMethods: PaymentMethod[];
  keys: KeyRecord[];
  owner: OwnerSnapshot;
}
