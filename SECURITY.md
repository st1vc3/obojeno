# Security

## Reporting a vulnerability

Please report vulnerabilities privately, not in a public issue: use
[Report a vulnerability](https://github.com/st1vc3/obojeno/security/advisories/new)
on GitHub. Reports are answered as soon as possible. Once a fix is released,
the advisory is published, with credit to you if you like.

## Supported versions

Fixes go into the latest release. Update to it before reporting.

## What obojeno does that matters for security

- **Network:** the menu bar app asks the GitHub API for the latest release,
  once a day when automatic update checks are on, and when you ask it to.
  Nothing else goes over the network, and nothing about you or your files is
  sent.
- **Local socket:** the daemon listens on
  `~/.local/state/obojeno/obojeno.sock`. The folder is readable only by you,
  the socket accepts only your user, and requests are limited in size and
  time.
- **Files and commands:** obojeno writes theme files where `config.toml`
  says, and runs the reload commands it lists (`reload = [...]` for custom
  targets). Anyone who can edit your config can already run commands as you,
  so this is by design. Report a way for something else to make obojeno write
  or run something.
- **Images:** the daemon decodes images with macOS ImageIO in short-lived
  helper processes, so an image that crashes the decoder takes down only the
  helper.
