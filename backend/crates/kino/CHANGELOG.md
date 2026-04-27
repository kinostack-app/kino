# Changelog

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
