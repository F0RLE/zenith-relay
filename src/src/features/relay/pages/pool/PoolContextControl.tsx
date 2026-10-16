import { useCallback, useEffect, useRef, useState } from "react";
import { Maximize2, Minimize2, Settings2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { ModelContextPolicy } from "../../api/types";
import { contextWindowTokens, customContextMaximum, knownContextMaximum, MIN_CUSTOM_CONTEXT } from "./modelContextPolicy";

export type { ModelContextPolicy } from "../../api/types";

export function PoolContextControl({
  value,
  maximumTokens,
  defaultTokens,
  resetRevision,
  disabled,
  saving,
  onChange,
  className = "",
}: {
  value: ModelContextPolicy;
  maximumTokens?: number | undefined;
  defaultTokens?: number | undefined;
  resetRevision: number;
  disabled: boolean;
  saving: boolean;
  onChange: (value: ModelContextPolicy) => void;
  className?: string;
}) {
  const { t, i18n } = useTranslation();
  const maximum = knownContextMaximum(maximumTokens);
  const selectedTokens = contextWindowTokens(value, maximum, defaultTokens);
  const customMaximum = customContextMaximum(maximum);
  const customSeed = Math.min(customMaximum, Math.max(MIN_CUSTOM_CONTEXT, selectedTokens ?? MIN_CUSTOM_CONTEXT));
  const number = (tokens: number) => tokens.toLocaleString(i18n.language);
  const controlRef = useRef<HTMLDivElement>(null);
  const groupRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const cancelBlur = useRef(false);
  const [customTokens, setCustomTokens] = useState(String(
    value.mode === "custom" ? value.tokens : customSeed,
  ));
  const selectedKey = value.mode;
  const parsedTokens = Number(customTokens);
  const customTokensValid = Number.isInteger(parsedTokens) && parsedTokens >= MIN_CUSTOM_CONTEXT && parsedTokens <= customMaximum;
  const savedTokens = value.mode === "custom" ? value.tokens : customSeed;
  const range = t("pool.contextPolicies.customTokensRange", { min: number(MIN_CUSTOM_CONTEXT), max: number(customMaximum) });
  const submittedTokens = useRef(savedTokens);
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;
  const commitCustomTokens = useCallback(() => {
    if (!disabled && selectedKey === "custom" && customTokensValid && parsedTokens !== submittedTokens.current) {
      submittedTokens.current = parsedTokens;
      onChangeRef.current({ mode: "custom", tokens: parsedTokens });
    }
  }, [selectedKey, disabled, customTokensValid, parsedTokens]);
  useEffect(() => {
    const requestedTokens = submittedTokens.current;
    submittedTokens.current = savedTokens;
    // An acknowledgement must not erase newer typing, but a rejected optimistic
    // save must restore the confirmed value instead of retrying every debounce.
    if (document.activeElement !== inputRef.current || requestedTokens !== savedTokens) setCustomTokens(String(savedTokens));
  }, [selectedKey, savedTokens, resetRevision]);
  useEffect(() => {
    if (selectedKey !== "custom" || disabled || !customTokensValid || parsedTokens === submittedTokens.current) return;
    const timer = window.setTimeout(commitCustomTokens, 450);
    return () => window.clearTimeout(timer);
  }, [selectedKey, disabled, customTokensValid, parsedTokens, commitCustomTokens]);
  const options = [
    { policy: "short" as const, icon: Minimize2, label: t("pool.contextPolicies.short") },
    { policy: "extended" as const, icon: Maximize2, label: t("pool.contextPolicies.extended") },
    { policy: "custom" as const, icon: Settings2, label: t("pool.contextPolicies.custom") },
  ];
  useEffect(() => {
    const group = groupRef.current;
    if (!controlRef.current?.contains(document.activeElement)) return;
    if (!group) return;
    group.querySelector<HTMLButtonElement>("[aria-checked='true']")?.focus();
  }, [selectedKey]);
  const move = (offset: number) => {
    const index = options.findIndex(({ policy }) => policy === selectedKey);
    const next = options[Math.min(options.length - 1, Math.max(0, index + offset))];
    if (next?.policy === "custom" && customMaximum < MIN_CUSTOM_CONTEXT) return;
    if (next && next.policy !== selectedKey && !disabled) onChange(next.policy === "custom"
      ? { mode: "custom", tokens: customSeed }
      : { mode: next.policy });
  };
  return <div
    ref={controlRef}
    className={`pool-context-control pool-model-control${className ? ` ${className}` : ""}`}
    aria-busy={saving}
    data-context-policy={selectedKey}
  >
    <div
      ref={groupRef}
      className="pool-context-options"
      role="radiogroup"
      aria-label={t("pool.context")}
      onKeyDown={(event) => {
        if (disabled) return;
        if (event.key === "ArrowRight" || event.key === "ArrowDown") { event.preventDefault(); move(1); }
        else if (event.key === "ArrowLeft" || event.key === "ArrowUp") { event.preventDefault(); move(-1); }
        else if (event.key === "Home") { event.preventDefault(); onChange({ mode: "short" }); }
        else if (event.key === "End") {
          event.preventDefault();
          onChange(customMaximum < MIN_CUSTOM_CONTEXT ? { mode: "extended" } : { mode: "custom", tokens: customSeed });
        }
      }}
    >
      {options.map(({ policy, icon: Icon, label }) => {
        const active = policy === selectedKey;
        return <button
          key={policy}
          type="button"
          role="radio"
          className={active ? "active" : ""}
          aria-checked={active}
          aria-label={label}
          tabIndex={active ? 0 : -1}
          disabled={disabled || (policy === "custom" && customMaximum < MIN_CUSTOM_CONTEXT)}
          data-relay-tooltip={`${label} · ${contextWindowTokens(policy === "custom" ? { mode: "custom", tokens: customSeed } : { mode: policy }, maximum, defaultTokens)?.toLocaleString(i18n.language) ?? t("pool.contextPolicies.unknown")} ${t("pool.contextPolicies.tokens")}`}
          onClick={() => {
            if (disabled || policy === selectedKey) return;
            if (policy === "custom") {
              onChange(value.mode === "custom" ? value : { mode: "custom", tokens: customSeed });
            } else {
              onChange({ mode: policy });
            }
          }}
        ><Icon aria-hidden /></button>;
      })}
    </div>
    <div className="pool-context-budget" data-relay-tooltip={maximum === undefined ? t("pool.contextPolicies.maximumUnknown") : t("pool.contextPolicies.maximum", { tokens: number(maximum) })}>
    {value.mode === "custom" ? <><input
      ref={inputRef}
      aria-label={t("pool.contextPolicies.customTokens")}
      data-relay-tooltip={range}
      aria-invalid={!customTokensValid}
      aria-description={customTokensValid ? undefined : range}
      className="pool-context-token-input"
      type="number"
      min={MIN_CUSTOM_CONTEXT}
      max={customMaximum}
      step={1}
      value={customTokens}
      disabled={disabled}
      onChange={(event) => setCustomTokens(event.currentTarget.value)}
      onBlur={() => {
        if (cancelBlur.current) cancelBlur.current = false;
        else commitCustomTokens();
      }}
      onKeyDown={(event) => {
        if (event.key === "Enter") {
          event.preventDefault();
          commitCustomTokens();
          event.currentTarget.blur();
        } else if (event.key === "Escape") {
          event.preventDefault();
          event.stopPropagation();
          cancelBlur.current = true;
          submittedTokens.current = value.tokens;
          setCustomTokens(String(value.tokens));
          event.currentTarget.blur();
        }
      }}
    />{customTokensValid && <strong aria-hidden="true" className="pool-context-token-value pool-context-token-preview">{number(parsedTokens)}</strong>}</> : <strong className="pool-context-token-value">{selectedTokens === undefined ? t("pool.contextPolicies.unknown") : number(selectedTokens)}</strong>}
    </div>
  </div>;
}
