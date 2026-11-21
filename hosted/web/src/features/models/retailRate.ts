/** USD per million, not wallet microUSD. The server serializes numeric(30,12) as a string. */
export function canonicalRetailRate(value: unknown): string | null {
  if (typeof value !== "string" || !/^(?:0|[1-9]\d{0,17})(?:\.\d{1,12})?$/.test(value) || value.trim() !== value) return null;
  const [whole, fraction = ""] = value.split(".");
  return `${whole}.${fraction.padEnd(12, "0")}`;
}

export function modelRate(value: unknown): string | null {
  return canonicalRetailRate(value)?.replace(/0+$/, "").replace(/\.$/, "") ?? null;
}

export function higherRate(left: string, right: string) {
  const units = (value: string) => BigInt(canonicalRetailRate(value)!.replace(".", ""));
  return units(left) > units(right);
}
