# Security

Carnet keeps people's notes, and its sync is meant to keep them private even from the server that carries them. Reports of anything that could expose, alter or lose notes are very welcome.

## Reporting a vulnerability

Please report it privately, not in a public issue or pull request:

- through GitHub's private vulnerability reporting: [Report a vulnerability](https://github.com/qxuken/carnet/security/advisories/new) on qxuken/carnet, or
- by email to [me@qxuken.dev](mailto:me@qxuken.dev).

Say what you found, which version or commit, and how to reproduce it if you can. You'll get a reply acknowledging the report, and we'll agree on a fix and on when to disclose it. Credit in the release notes is yours if you want it.

## Scope

In scope:

- **The desktop app**: how it reads and writes the notes folder, pictures and the clipboard, and how it keeps sync keys in the system's keychain.
- **The sync protocol and its cryptography** (`crates/carnet-sync`): encryption of notes, pictures and the vault, logins, pairing, recovery words, passwords (OPAQUE) and passkeys.
- **`carnet-server`**: authentication, isolation between accounts, rate limits, signed links to pictures, storage, backups and migration.
- **The web app** (`web/` and `crates/carnet-web`): what it keeps in the browser, and how it pairs and signs in.

Out of scope: problems that need an attacker who already controls your unlocked device, and the configuration of a particular server (its proxy, TLS or host), unless Carnet's defaults or documentation lead to the mistake.

## Threat model

[docs/SYNC.md](docs/SYNC.md) §7 describes the encryption, how devices get in, and what a server can and cannot see. In short, the server stores and relays sealed records and never holds a key that opens them, but it does see metadata such as accounts, devices, how many notes there are, their sizes, and when they change. A way for the server, or anyone between a device and the server, to learn more than §7 says is a vulnerability.
