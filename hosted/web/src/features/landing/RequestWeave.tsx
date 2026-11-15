import { useEffect, useRef } from "react";

const THREADS = 22;
const bands = [
  { start: 48, middle: 185, end: 260, color: "mint" },
  { start: 256, middle: 134, end: 38, color: "cream" },
  { start: 162, middle: 170, end: 168, color: "sage" },
] as const;

function strand(band: typeof bands[number], index: number) {
  const offset = (index - (THREADS - 1) / 2) * 4;
  return `M-40 ${band.start + offset} C190 ${band.start + offset} 300 ${band.middle + offset} 510 ${band.middle + offset} S820 ${band.end + offset} 1240 ${band.end + offset}`;
}

/** An illustrative connection, never a measurement of live traffic. */
export function RequestWeave({ description, inputLabel, outputLabel }: {
  description: string; inputLabel: string; outputLabel: string;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const element = ref.current;
    if (!element) return;
    const media = matchMedia("(prefers-reduced-motion: reduce)");
    let visible = true;
    const sync = () => { element.dataset.motion = !media.matches && visible && !document.hidden ? "running" : "paused"; };
    const observer = new IntersectionObserver(([entry]) => { visible = Boolean(entry?.isIntersecting); sync(); });
    observer.observe(element);
    media.addEventListener("change", sync);
    document.addEventListener("visibilitychange", sync);
    sync();
    return () => {
      observer.disconnect();
      media.removeEventListener("change", sync);
      document.removeEventListener("visibilitychange", sync);
    };
  }, []);

  return <div className="request-weave" ref={ref} role="img" aria-label={description} data-motion="paused">
    {/* Keep the full vertical strand envelope; only horizontal edge cropping is intentional. */}
    <svg className="request-weave-art" viewBox="0 -20 1200 360" preserveAspectRatio="none" aria-hidden="true">
      {bands.map((band, index) => <g key={band.color} className={`weave-band weave-${band.color}`}>
        {Array.from({ length: THREADS }, (_, thread) => <path className="weave-thread" key={thread} d={strand(band, thread)} />)}
        <path className={`weave-request weave-request-${index}`} d={strand(band, 11)} pathLength="100" />
      </g>)}
    </svg>
    <div className="weave-labels" aria-hidden="true"><span>{inputLabel}</span><strong>Relay</strong><span>{outputLabel}</span></div>
  </div>;
}
