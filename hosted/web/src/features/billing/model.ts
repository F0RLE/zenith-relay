export interface PaymentMethod { id: string; label: string }
export interface TopUpRequest { microUsd: bigint; methodId: string }

export const topUpLimits = { min: 1, max: 1000, step: 1 } as const;
export const quickTopUpAmounts = [10, 25, 50, 100] as const;

export function isTopUpAmount(microUsd: bigint): boolean {
  return microUsd >= BigInt(topUpLimits.min) * 1_000_000n
    && microUsd <= BigInt(topUpLimits.max) * 1_000_000n
    && microUsd % 1_000_000n === 0n;
}

export function parseTopUpAmount(value: string): bigint | null {
  const amount = Number(value);
  if (!value.trim() || !Number.isSafeInteger(amount)) return null;
  const microUsd = BigInt(amount) * 1_000_000n;
  return isTopUpAmount(microUsd) ? microUsd : null;
}
