# Relay Hosted

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

People connecting AI models to their own applications through an API, including
people who need a short first-use explanation. The public entry must make the
service understandable without requiring knowledge of the desktop Relay product.

## Product Purpose

Explain Relay Hosted and take visitors into the customer cabinet. The cabinet
contains models, API keys, usage and billing for API clients.

## Capabilities and Constraints

The current implementation is a React/TypeScript frontend with synthetic
development fixtures isolated from production builds. Production serves the
closed landing, public guide, operator policy links, local first-owner setup and
passkey sign-in to a closed service overview with sign-out.
An optional OIDC client lets the owner approve restricted customer access with
a fresh passkey. Approved users can sign in, read their wallet, issue and manage
their own one-time-disclosed keys, inspect request receipts and the matching
wallet entries, see published models and sign out. The owner admits verified
inventory in Pool, edits prices inline and publishes to the current testers.
Connections lists saved API sources, proxy assignments and current verified models.
Drafts and credential details never enter the customer catalog.
Public registration and paid operations are not open. Real account status, prices and
availability require connected backend evidence. Hosted owns its credentials
and controls separately from Desktop.

The public entry supports Russian, English and Simplified Chinese, light/dark
themes, keyboard navigation and reduced motion. In the development preview, Start and the cabinet links open
`/app/overview` directly, without a first-use dialog. A quieter Models & pricing
action opens `/app/models`. All four initial FAQ answers stay visible.

## Brand Commitments

Relay name; green, warm neutral and charcoal palette; short, plain-language copy.
The public landing, customer and owner workspaces, and supporting screens share
one visual identity.
The desktop product and backend behavior remain outside that design scope.
Use concise interface copy and attributable product claims.

## Evidence on Hand

The working frontend and existing text in `src/features/landing` and
`src/i18n/messages`; the maintained contracts in `README.md` and
`../../docs/hosted/HOSTED_RELAY_WEB_DESIGN.md`.

## Product Principles

- Explain the service before asking the visitor to act.
- Keep the path into the cabinet and optional help direct.
- Make a clear distinction between illustration and real account data.
- Keep the public surface independent from local desktop functionality.

## Accessibility & Inclusion

Readable contrast, visible keyboard states, reachable touch controls, no layout
overflow at 320px, and a complete reduced-motion presentation.
