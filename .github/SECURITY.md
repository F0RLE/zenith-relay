# Reporting a security vulnerability

Use GitHub's [private vulnerability report](https://github.com/F0RLE/zenith-relay/security/advisories/new)
for suspected security issues in Zenith Relay. Keep exploit details and private
data in that report. Use the repository's issue tracker for ordinary bugs.

## What to include

- The affected Relay version or commit, operating system, and relevant settings.
- A description of the impact and the conditions needed to reproduce it.
- Minimal reproduction steps using synthetic accounts, tokens, and data.
- Redacted logs or screenshots only when they help explain the issue.

Share synthetic data and redacted diagnostics only. Keep working credentials,
cookies, sessions, authorization headers, private prompts and other people's data
private. Test only accounts and systems you own or are authorized to assess.

## Versions and follow-up

If practical, check whether the issue also affects the
[latest stable release](https://github.com/F0RLE/zenith-relay/releases/latest).
Reports from older versions are welcome; include the exact version rather than
assuming it is still affected or already fixed.

The maintainer uses the private report to confirm affected versions, discuss a
fix or mitigation, and coordinate disclosure. Keep further reproduction details
in that private conversation and discuss disclosure and release timing there.
