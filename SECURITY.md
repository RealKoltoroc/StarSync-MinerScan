# Security Policy

If you discover a security issue, please do not include private logs, account data, API credentials or personal paths in a public issue.

For ordinary bugs, open an issue in the public repository:

**https://github.com/RealKoltoroc/StarSync-MinerScan**

For security-sensitive reports, use the private security-reporting mechanism offered by the repository host once enabled.

## Scope

Security-sensitive areas include:

- screen-capture boundaries
- local file handling / ZIP import
- external provider HTTP requests and cache handling
- global keyboard hooks
- tray/native Windows message handling
- HTML report generation and embedded media

MinerScan intentionally does not inject into Star Citizen, read game memory or intercept game network traffic.
