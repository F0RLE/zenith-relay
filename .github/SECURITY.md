# Reporting a security vulnerability

Use GitHub's [private vulnerability report](https://github.com/F0RLE/zenith-relay/security/advisories/new)
for suspected security issues in Zenith Relay. Do not open a public issue or
pull request containing exploit details or private data. Ordinary bugs can use
the repository's issue tracker.

## What to include

- The affected Relay version or commit, operating system, and relevant settings.
- A description of the impact and the conditions needed to reproduce it.
- Minimal reproduction steps using synthetic accounts, tokens, and data.
- Redacted logs or screenshots only when they help explain the issue.

Never attach working credentials, cookies, session files, authorization headers,
private prompts, or other people's data. Do not test against accounts or systems
you do not own or have permission to assess.

## Versions and follow-up

If practical, check whether the issue also affects the
[latest stable release](https://github.com/F0RLE/zenith-relay/releases/latest).
Reports from older versions are welcome; include the exact version rather than
assuming it is still affected or already fixed.

The maintainer uses the private report to confirm affected versions, discuss a
fix or mitigation, and coordinate disclosure. Keep further reproduction details
in that private conversation. This policy does not promise a response deadline
or a release date.
