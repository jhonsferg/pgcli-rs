# Security Policy

## Supported Versions

Only the latest release receives security fixes.

| Version | Supported |
| ------- | --------- |
| 0.x     | yes       |

## Reporting a Vulnerability

**Please do not open a public GitHub issue for security vulnerabilities.**

Report security issues privately via GitHub's built-in mechanism:

1. Go to the Security tab of this repository.
2. Click **"Report a vulnerability"**.
3. Fill in the details: affected versions, reproduction steps, and potential impact.

You will receive an acknowledgement within **72 hours** and a resolution timeline
within **7 days** for critical issues.

## Scope

- SQL injection or data exfiltration via crafted server responses
- Path traversal in `.pgpass` file parsing or `\copy` meta-command
- Credential or connection-string leakage in logs or error messages
- TLS downgrade attacks or certificate validation bypass
- Remote code execution via crafted PostgreSQL wire protocol messages

## Out of Scope

- Vulnerabilities in PostgreSQL servers themselves (report to the [PostgreSQL team](https://www.postgresql.org/support/security/))
- Social engineering or phishing
- Issues in systems that `pgcli-rs` does not control (e.g. OS keychain, network infrastructure)
