# Changelog

## [Unreleased]

### Added

- Added GitHub Packages distribution as `@at-inc/pi-durable` with matching fork Chord and AI dependencies pinned to the exact release version. Consumers can retain upstream import names with npm aliases.

### Fixed

- Reserve tokenizer margin for summary requests before pinning the native cut and output budget; reject requests that still cannot fit without calling the model.
- Reject native and hook-supplied summaries that exceed the pinned output budget before placement, while preserving source entries and recording rejected provider spend.
- Avoid resummarizing unchanged history retained by safety selection unless new context or explicit focus exists.

## [1.0.0] - 2026-10-01

### Added

- Initial release of `@earendil-works/pi-durable`, a durable agent harness. See the [README](README.md) and the [design document](https://github.com/earendil-works/pi/blob/main/packages/durable/docs/spec.md).
