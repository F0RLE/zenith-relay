import type en from "./messages/en.json";

export type MessageKey = keyof typeof en;
export type Messages = Record<MessageKey, string>;
export type MessageParams = Readonly<Record<string, string | number>>;
export type Translator = (key: MessageKey, params?: MessageParams) => string;
