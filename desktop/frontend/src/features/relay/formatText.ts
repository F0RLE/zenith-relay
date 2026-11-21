/** Text formatting shared by clients with independent localization runtimes. */
export type FormatText = (...args: [key: string] | [key: string, values: Readonly<Record<string, unknown>>]) => string;
