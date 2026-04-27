# Kino

Single-binary self-hosted media automation and streaming server.
Discovers, acquires, organises, transcodes, streams, and casts
your media library from one Rust binary — no docker-compose stack,
no service mesh, no plugin install.

- **Site**: <https://kinostack.app>
- **Docs**: see [`docs/`](./docs/) — start at [`docs/README.md`](./docs/README.md)
- **Subsystems**: shipped reference at [`docs/subsystems/`](./docs/subsystems/),
  planned at [`docs/roadmap/`](./docs/roadmap/)

## Status

Pre-release. Not yet published to package channels — see
[`docs/roadmap/21-cross-platform-deployment.md`](./docs/roadmap/21-cross-platform-deployment.md)
for the distribution plan.

## Install (preview)

Once the first release lands, install through your platform's package
manager:

```sh
brew install kino                    # macOS Homebrew Formula (CLI / headless)
brew install --cask kino             # macOS Homebrew Cask (.app + tray)
winget install kino                  # Windows
sudo apt install kino                # Debian/Ubuntu (after adding the kino repo)
yay -S kino-bin                      # Arch
docker pull ghcr.io/kinostack-app/kino   # Anywhere
curl -fsSL https://kinostack.app/install.sh | sh    # Linux/macOS
irm https://kinostack.app/install.ps1 | iex         # Windows
```

Direct downloads (`.msi`, `.dmg`, `.deb`, `.rpm`, `.tar.gz`) attach to
each [GitHub Release](https://github.com/kinostack-app/kino/releases).

## Build from source

```sh
cd backend
just fix-ci          # Lint + format check
just test            # Run tests
just build-release   # Optimised binary
```

Headless build (no tray, smaller image — what the Docker / Pi image
ships):

```sh
cargo build --release -p kino --no-default-features
```

## Contributing

Read [`CLAUDE.md`](./CLAUDE.md) for the project layout, dev container
setup, and quality-gate scripts.

## License

GPL-3.0-or-later — see [`LICENSE`](./LICENSE).
