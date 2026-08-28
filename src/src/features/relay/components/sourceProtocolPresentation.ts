import { Braces, Globe2, MessageSquareText, Sparkles, type LucideIcon } from "lucide-react";
import type { SourceAdapter, SourceWireApi } from "../api/types";

export const protocolPresentation = {
  responses: { icon: Sparkles, endpoint: "/responses" },
  messages: { icon: MessageSquareText, endpoint: "/messages" },
  chat_completions: { icon: Braces, endpoint: "/chat/completions" },
  gemini: { icon: Globe2, endpoint: "/v1beta/models/{model}:generateContent" },
} as const;

export type SimpleRouteCard = {
  id: string;
  wireApi: SourceWireApi;
  adapter: SourceAdapter;
  icon: LucideIcon;
  titleKey: string;
  subtitleKey: string;
};

export const simpleRouteCards: readonly SimpleRouteCard[] = [
  {
    id: "openai",
    wireApi: "responses",
    adapter: "native",
    icon: Sparkles,
    titleKey: "sources.simpleRouteCards.openai.title",
    subtitleKey: "sources.simpleRouteCards.openai.protocol",
  },
  {
    id: "anthropic",
    wireApi: "messages",
    adapter: "native",
    icon: MessageSquareText,
    titleKey: "sources.simpleRouteCards.anthropic.title",
    subtitleKey: "sources.simpleRouteCards.anthropic.protocol",
  },
  // Google sources use the provider's native Gemini contract by default. The
  // Responses-to-Gemini bridge remains available in the advanced route matrix.
  {
    id: "google",
    wireApi: "gemini",
    adapter: "native",
    icon: Globe2,
    titleKey: "sources.simpleRouteCards.google.title",
    subtitleKey: "sources.simpleRouteCards.google.protocol",
  },
] as const;
