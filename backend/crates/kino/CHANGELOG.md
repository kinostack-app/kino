# Changelog

## [0.3.1](https://github.com/kinostack-app/kino/compare/v0.3.0...v0.3.1) (2026-04-29)


### Fixed

* **release:** complete v0.3.1 trigger + msix release notes ([#7](https://github.com/kinostack-app/kino/issues/7)) ([7d5e30f](https://github.com/kinostack-app/kino/commit/7d5e30f0d1bf2337e697157b2f9c3bd30ba13fde))

## [0.3.0](https://github.com/kinostack-app/kino/compare/v0.2.1...v0.3.0) (2026-04-29)


### Added

* cross-OS install polish + beta-testing bug fixes ([498ffb4](https://github.com/kinostack-app/kino/commit/498ffb4372078a7dda1cd9548cfc1e4a54ab8362))
* **fs:** graceful path fallback, places endpoint, EACCES distinction ([7a2c58b](https://github.com/kinostack-app/kino/commit/7a2c58b133a219eee8a6e3f33e3a152919a10d6b))
* **fs:** polished folder picker — breadcrumbs, mounts, search, mkdir ([14463b8](https://github.com/kinostack-app/kino/commit/14463b87b33a313a6ec0ee0bfb7f8390871e1768))
* **indexers:** on-demand definitions refresh + remove blocking startup fetch ([5feec5b](https://github.com/kinostack-app/kino/commit/5feec5b939854b1fc05ecc11f97792e68f2fb10e))
* **launcher:** kino open subcommand + .desktop launcher ([c489042](https://github.com/kinostack-app/kino/commit/c489042bb8985450249568bea6f6b32398e45872))
* **mdns:** collision detection + dev-container hostname split ([e8d84ae](https://github.com/kinostack-app/kino/commit/e8d84ae6b9e173e0d83ce570b729d34b527f4a87))
* **msix:** Microsoft Store / sideload MSIX channel ([#5](https://github.com/kinostack-app/kino/issues/5)) ([72bbd3a](https://github.com/kinostack-app/kino/commit/72bbd3a097a412fd29cd2ac041f3846eaffa20c0))
* **picker:** stable layout, EACCES feedback, kino setup-permissions, docs ([17b2b74](https://github.com/kinostack-app/kino/commit/17b2b74873bfef96813171d5f5686a32ffd69604))
* **setup:** platform-aware copy + path validation + auto-create dirs ([fb3203c](https://github.com/kinostack-app/kino/commit/fb3203c2919872d0f2c37d8479166d686baca314))
* **wizard:** vertical rail layout, consent-gated catalogue, tarball fetch + UX polish ([81a83ab](https://github.com/kinostack-app/kino/commit/81a83abf161fbd220c23ff053b1ac908aefb5fd8))


### Fixed

* **cast:** dedupe mDNS discovery logs ([e9fa418](https://github.com/kinostack-app/kino/commit/e9fa4188bcfafa7a9aa46a4e88fa85d5a329b7fd))
* mDNS whitelist + listen_port test assertion ([7a6c043](https://github.com/kinostack-app/kino/commit/7a6c043c8f9bc79ca716b1a911475f562d8f1145))
* **mdns:** replace in-process responder with shell-out to system tools ([75be73b](https://github.com/kinostack-app/kino/commit/75be73b0aaab28edf4cf5a87598083ea1c44d19a))
* **release:** regenerate wix/main.wxs to match product-icon metadata ([9d0a190](https://github.com/kinostack-app/kino/commit/9d0a1906da67eb75079d95d40e6eb8bbde44e7d4))
* TMDB test endpoint reads DB on demand + /api/v1/health is public ([a40e158](https://github.com/kinostack-app/kino/commit/a40e158796e6827f3cbb6a2a9758bcc7c0f5f480))
* TMDB warning reads DB on demand; mask stored secrets in Settings ([9941903](https://github.com/kinostack-app/kino/commit/9941903cc0f76306d8fc15a80f55ca9418b92316))

## [0.2.1](https://github.com/kinostack-app/kino/compare/v0.2.0...v0.2.1) (2026-04-27)


### Fixed

* **release:** boolish KINO_NO_OPEN_BROWSER + smoke-test + table-format download UI ([e6864bd](https://github.com/kinostack-app/kino/commit/e6864bdeccaf190124b0953db6746376f6fa4648))

## [0.2.0](https://github.com/kinostack-app/kino/compare/v0.1.0...v0.2.0) (2026-04-27)


### Added

* **kino:** embed the React SPA in the binary, serve at / via rust-embed ([0377e56](https://github.com/kinostack-app/kino/commit/0377e56cbf4bd6c05346821c2359fac239232f76))


### Fixed

* **packaging:** service descriptor flag-position bug + apt purge cleanup ([4aac052](https://github.com/kinostack-app/kino/commit/4aac052a0069ad95d9593c69b89c696e4b974bab))

## 0.1.0 (2026-04-27)


### Added

* initial public commit ([7a05b63](https://github.com/kinostack-app/kino/commit/7a05b6373da9de380601b43bd997ebb167d1b1f7))


### Fixed

* **linux-packages:** cargo-deb 3.x is package-relative, not workspace-relative ([e087501](https://github.com/kinostack-app/kino/commit/e0875016160af9b9d630219d0af702277b29b18a))
* **linux-packages:** cross-arch (aarch64) deb builds — drop $auto + --no-strip ([2428089](https://github.com/kinostack-app/kino/commit/2428089a5b9f73ff6a5b903f67f1607d285cc46b))
