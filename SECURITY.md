# Security policy

## Report a vulnerability

Please report it [privately on GitHub](https://github.com/sventmueller/tapa/security/advisories/new), not in an issue: issues are public. Keep it private until a fix is out.

Name the Tapa version and the macOS version and describe how to reproduce it. Attach a small sample if one triggers it, such as a file of escape sequences, a repository or a project folder.

## What counts

Tapa treats program output, project folders and pictures from them as untrusted. Anything that breaks a promise under [Trust](README.md#trust) or [Privacy and security](README.md#privacy-and-security) is a vulnerability, for example output that injects input or reads the clipboard without permission, a workspace that runs a program or loads agent hooks before you trusted it, history on disk that is not encrypted, a request to the network you did not ask for, or a picture that gets out of the decoder's sandbox.

## Supported versions

Security fixes go into the latest release only.
