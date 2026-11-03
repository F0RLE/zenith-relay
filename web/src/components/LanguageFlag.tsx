import type { ReactNode } from "react";
import type { Locale } from "../i18n/catalog";
import { localeDefinition } from "../i18n/catalog";

const star = "0,-1 .2245,-.309 .9511,-.309 .3633,.118 .5878,.809 0,.382 -.5878,.809 -.3633,.118 -.9511,-.309 -.2245,-.309";

const flags: Partial<Record<Locale, ReactNode>> = {
  ru: <><path fill="#fff" d="M0 0h30v20H0z" /><path fill="#0039a6" d="M0 6.667h30v6.666H0z" /><path fill="#d52b1e" d="M0 13.333h30V20H0z" /></>,
  en: <>
    <path fill="#fff" d="M0 0h30v20H0z" />
    {Array.from({ length: 7 }, (_, row) => <rect key={row} fill="#b22234" y={row * 40 / 13} width="30" height={20 / 13} />)}
    <path fill="#3c3b6e" d="M0 0h12v10.769H0z" />
    {Array.from({ length: 9 }, (_, row) => Array.from({ length: row % 2 ? 5 : 6 }, (_, column) =>
      <polygon key={`${row}-${column}`} points={star} fill="#fff" transform={`translate(${1 + column * 2 + row % 2} ${(row + 1) * 1.077}) scale(.52)`} />))}
  </>,
  "zh-CN": <>
    <path fill="#de2910" d="M0 0h30v20H0z" />
    <polygon points={star} fill="#ffde00" transform="translate(5 5) scale(3)" />
    {[[10, 2, -121], [12, 4, -98], [12, 7, -74], [10, 9, -51]].map(([x, y, angle]) =>
      <polygon key={y} points={star} fill="#ffde00" transform={`translate(${x} ${y}) rotate(${angle})`} />)}
  </>,
};

export function LanguageFlag({ locale }: { locale: Locale }) {
  return <svg className="language-flag" data-locale={locale} viewBox="0 0 30 20" width="24" height="16" aria-hidden="true" focusable="false">{flags[locale] ?? <text x="15" y="14" textAnchor="middle" fill="currentColor" fontSize="10">{localeDefinition(locale).shortLabel}</text>}</svg>;
}
