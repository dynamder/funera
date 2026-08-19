# Security Policy

## Supported versions

Only the latest release is actively supported. Security fixes are released as a
new patch version; older versions are not back-ported.

## Security model

funera is under active development. The `security` and `sandbox` features are
**experimental and not yet hardened** — they must not be relied upon to contain
untrusted code. See the README warning for details.

Anything that could compromise a host, leak secrets, or escape the configured
sandbox is in scope.

## Reporting a vulnerability

Please **do not** open a public issue for security vulnerabilities.

Report them privately via GitHub's
[private vulnerability reporting](https://github.com/dynamder/funera/security/advisories/new).
If that is unavailable, contact the maintainers directly.

Please include:

- the affected version(s);
- a minimal reproduction or a precise description of the vulnerable path;
- any relevant feature flags (`security`, `sandbox`, …);
- the impact you believe it has.

We will acknowledge your report promptly and work with you on a fix and a
coordinated disclosure. Credit is given to reporters who wish it.
