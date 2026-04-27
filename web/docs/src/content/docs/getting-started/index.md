---
title: Getting started
description: Install kino on your platform, then run through the first-run setup wizard.
---

kino ships as a single native binary on every supported platform —
no docker-compose stack, no service mesh, no plugin install.

## Install

| Platform | Page |
|---|---|
| Linux | [Install on Linux](./install-linux) |
| macOS | [Install on macOS](./install-macos) |
| Windows | [Install on Windows](./install-windows) |
| Raspberry Pi | [Install on Raspberry Pi](./install-raspberry-pi) |
| Docker | [Install with Docker](./install-docker) |

The new-to-kino path is the [Quickstart](./quickstart) — a five-
minute walkthrough that takes you from "binary not installed" to
"first item in the library."

## After install

Once kino is running, open `http://localhost:8080` (or
`http://<host>:8080` if you're running it on another machine) and
work through the [first-run setup](./first-run-setup). It collects
the handful of things kino needs to know to run:

- Where your media library lives + where to stage downloads
- A free TMDB Read Access Token (for posters, descriptions, etc.)
- Which languages you want releases in
- One or more indexers — pick from the built-in catalogue, or
  point at any Torznab endpoint

Every choice in the wizard is reversible from
[Settings](https://kinostack.app/) later.

## Stuck?

See [Troubleshooting](../troubleshooting) for first-run issues —
the answer to "why doesn't `localhost:8080` respond" is in there
five different ways.
