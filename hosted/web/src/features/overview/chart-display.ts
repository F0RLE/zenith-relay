/** Display scales only: source counters, money and bucket boundaries stay exact. */
export function chartCeiling(values: readonly (number | null)[], unit: string) {
  const maximum = Math.max(0, ...values.filter((value): value is number => value !== null && Number.isFinite(value)));
  const minimumStep = unit === "USD" ? .000001 : unit === "requests" ? 1 : 0;
  if (maximum === 0) return minimumStep ? minimumStep * 2 : 1;
  const rawStep = maximum / 2;
  const magnitude = 10 ** Math.floor(Math.log10(rawStep));
  const factor = [1, 2, 2.5, 5, 10].find(value => value * magnitude >= rawStep)!;
  let step = factor * magnitude;
  if (minimumStep) step = Math.ceil(Number((step / minimumStep).toPrecision(12))) * minimumStep;
  return step * 2;
}

export function chartLinePath(values: readonly (number | null)[], ceiling: number) {
  let path = "", connected = false;
  values.forEach((value, index) => {
    if (value === null) { connected = false; return; }
    path += `${connected ? "L" : "M"}${(index + .5) / values.length * 100},${(1 - value / ceiling) * 100} `;
    connected = true;
  });
  return path;
}
