import type { CandidateRuntimeSnapshot } from "./api/types";
import { runtimeCandidateForMember } from "./routingOrder";

type CandidateKind = CandidateRuntimeSnapshot["kind"];
type CandidateIdentity = { candidateId: string; kind?: CandidateKind };

/** Index one snapshot, preserving candidate order and source/protocol grouping. */
export function indexCandidatesByMember<T extends CandidateIdentity>(candidates: Iterable<T>) {
  const accounts = new Map<string, T[]>();
  const sources = new Map<string, T[]>();
  const empty: T[] = [];
  const append = (index: Map<string, T[]>, id: string, candidate: T) => {
    const group = index.get(id);
    if (group) group.push(candidate);
    else index.set(id, [candidate]);
  };
  for (const candidate of candidates) {
    const kind = candidate.kind;
    // Activity events lack kind; exact account IDs and source prefixes still
    // follow the same membership rules as their eventual runtime snapshot.
    if (kind !== "api_source") append(accounts, candidate.candidateId, candidate);
    if (kind === "oauth_account") continue;
    let id = candidate.candidateId;
    while (true) {
      append(sources, id, candidate);
      const separator = id.lastIndexOf("::");
      if (separator < 0) break;
      id = id.slice(0, separator);
    }
  }
  return (memberId: string, kind: CandidateKind) => (kind === "oauth_account" ? accounts : sources).get(memberId) ?? empty;
}

/** Keep aggregation in its existing owner; only narrow each lookup's input. */
export function createRuntimeCandidateLookup(order: readonly CandidateRuntimeSnapshot[]) {
  const candidatesFor = indexCandidatesByMember(order);
  return (
    memberId: string,
    kind: CandidateKind,
    protocolFilter: Parameters<typeof runtimeCandidateForMember>[3] = "all",
    sourceWireApi?: Parameters<typeof runtimeCandidateForMember>[4],
  ) => runtimeCandidateForMember(memberId, kind, candidatesFor(memberId, kind), protocolFilter, sourceWireApi);
}
