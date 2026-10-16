# Model catalog

Relay's default display order lives in [model-order.json](model-order.json),
validated by [model-order.schema.json](model-order.schema.json). It applies to
account and API inventories, pool lists, editors, and managed client catalogs.
This is Relay's presentation policy, not an official provider ranking.

## Order

Array positions define provider, family, class, and variant ranks. Models
compare by provider, family, class, descending numeric version, and variant.
Within the same variant, named aliases precede explicit parameter sizes;
larger sizes precede smaller ones.
An undated alias precedes dated snapshots; newer snapshots precede older ones.
The normalized model ID breaks remaining ties.

Providers follow the table order. Families and classes follow the JSON arrays:

| Provider | Main families and classes |
| --- | --- |
| OpenAI | Astra, Sol, Terra, Luna, base GPT |
| Anthropic | Fable, Opus, Sonnet, Haiku |
| Google | Gemini: Pro, Flash, Flash Lite; Gemma, Imagen, Veo |
| xAI | Grok |
| DeepSeek | V: Pro, Flash, base; R, Coder, Math, OCR |
| Z.ai | GLM: Prime, base, Turbo, Air, FlashX, Flash; vision, AutoGLM, image |
| Alibaba / Qwen | Qwen: Max, Plus, Turbo, Flash, open sizes; Coder, VL, Omni, MT, QwQ, QVQ, embedding, reranker, image, Wan |
| Moonshot AI / Kimi | Kimi: descending K generations, then Code, Instruct, Thinking and serving variants; legacy Moonshot |
| MiniMax | M, H: Max, base; legacy MiniMax, text, image, speech |
| Xiaomi | MiMo: Pro, base, Flash, Omni |
| Mistral | Mistral: Large, Medium, Small, Tiny; Magistral, Devstral, Leanstral, Codestral, Ministral, Mixtral, Pixtral, Voxtral, OCR |
| Tencent | Hunyuan / Hy; Hunyuan MT |
| NVIDIA | Nemotron: Ultra, Super, Lightning, Nano, base; safety, Switchyard, Cosmos |
| StepFun | Step: descending generations, then serving variants; Gelab Zero |
| Meta | Muse: Spark, Glimmer; Llama: Maverick, Scout, open sizes; Muse Image, Muse Voice, Llama Guard, Prompt Guard |
| ByteDance | Seed: Pro, Code, base, Turbo, Lite, Mini, Flash; Seed OSS, UI-TARS, Seedance |
| Perplexity | Sonar: Deep Research, Reasoning Pro, Pro Search, Pro, Reasoning, base; Decider, legacy PPLX, PPLX Embed |
| Cohere | Command A: Plus, base, Reasoning, Vision, Translate; Command R: Plus, base; legacy Command, North, Aya, Tiny Aya, Parse, Embed, Rerank |
| Amazon | Nova: Premier, Pro, Lite, Micro; Nova Sonic, Titan |
| Nous Research | Hermes: descending generations, then sizes and variants; base Llama versions are not Hermes generations |
| Inception | Mercury, Mercury Coder, Mercury Edit |

[OpenRouter's author request shares and model token rankings](https://openrouter.ai/rankings)
guide initial coverage; the additional groups use the model IDs and names in
its [public catalog](https://openrouter.ai/api/v1/models). Traffic changes do not
reorder the catalog automatically. Family and class positions are a stable
presentation policy, not a measured quality ranking across different tasks.

Provider aliases and inference patterns also live in the JSON. Anchored family
patterns use Rust regex syntax and optional named captures:

- `version`, `variant`, `suffix`: generation and variant components.
- `parameters`: a size such as `22m`, `27b`, `2.4t`, or `8x22b`, compared numerically.
- `snapshot`: a release tag in `YYMM` or `YYMMDD` format.
- `snapshot_month`: `MM-YYYY`, used by families such as Cohere Command.
- `snapshot_day`: `MMDD` or `MM-DD`, using the reference release year when
  available. Without it, the sortable date key retains year zero.

Full `YYYYMMDD` and `YYYY-MM-DD` snapshots are recognized before family parsing.
A dated or size-only ID can obtain its omitted generation from a reference
display name recognized by the same family and class. A date or parameter size
never becomes a generation.

Class aliases match complete hyphen-separated segments;
the longest match wins. A class with no aliases is the default for recognized
variants. Otherwise an unknown class follows listed classes.
Hosting modifiers after `:` belong to the variant and do not change its class
or hide a dated snapshot.

Unlisted providers follow alphabetically. Unlisted families use reference
family metadata, a name prefix, and numeric versions without a guessed
capability rank. Unknown identities remain available. The same rules work
without downloaded metadata; reference identity resolves hosted aliases when
available. Ambiguous reference leaves are never assigned a manufacturer by name.

Add a class or family by editing the JSON. Keep `revision: "1"` while preparing
the first release; increase it when changing an already released policy.
New numeric versions need no new entry. Keep `schemaVersion` for format changes.
An explicit saved model order overrides the default. Sorting neither rewrites
route IDs nor grants model access, capabilities, prices, or protocol support.
Direct native ChatGPT sign-in keeps the application's own catalog.

## Export for consumers

From the repository root:

```sh
bun scripts/build/cargo.mjs run --manifest-path crates/relay-core/Cargo.toml --locked --example export_model_catalog
```

The command refreshes Relay's public metadata sources and writes
`output/model-catalog/`. Pass a directory after `--` to choose another location.
The generated catalog is validated against its JSON Schema before an atomic
write. It generates four consumer files:

- `model-catalog.json`: ordered reference records with `position`, parsed
  `identity`, and validated `metadata`, including names and capabilities.
- `model-catalog.schema.json`: the export's JSON Schema.
- `model-order.json`: the policy used to calculate positions.
- `model-order.schema.json`: the policy's JSON Schema.

Consumers can read the `models` array as supplied, or filter their own available
IDs and retain the same relative order. `id` identifies a reference record;
`metadata.canonicalModelId` links a hosted alias when the reference supplies it.
`identity.provider` is the display manufacturer, whereas `metadata.provider`
retains the reference namespace. Do not merge account access or routes using
display identity alone.

`orderRevision` identifies the policy; `metadataRevision` identifies the merged
reference data. `sources` records each source's revision, fetch time, and stale
state: `models_dev`, `models_dev_details`, `openrouter`, and `litellm`.
Unavailable sources may leave a partial or stale export; inspect these fields.
No usable reference data makes the export fail.

The export includes only public reference records. Its `.cache/` directory is
local refresh state, not a consumer artifact. The command does not publish
files. Reference source URLs and capability contracts are documented in
[PLANNING.md](../docs/project/PLANNING.md#connections-discovery-and-metadata).
