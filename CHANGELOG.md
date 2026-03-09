# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

- 

## [0.0.1a6] - 2026-03-09

- Use basic json_dumps (#20)
- Extract basic auth for client requests (#19)

## [0.0.1a5] - 2026-03-09

- Improve ecosystem compatibility (#16)

## [0.0.1a4] - 2026-03-09

- More httpx compatibility and v0.0.1a4 (#11)

## [0.0.1a3] - 2026-03-07

### Added
- Expand data support (#9)

## [0.0.1a2] - 2026-03-07

### Added
- Expand `is_private_url` (#8)
- Add `is_unspecified` checks (#7)
- Add just bench (#4)
- Use justfile

### Fixed
- Prevent panic in `parse_digest_challenge` and add redirect policy (#5)
- Fix `TypeError` in `client.request` (#3)

## [0.0.1a1] - 2026-03-06

### Fixed
- Fix `TypeError` in `client.request`

## [0.0.1a0] - 2026-03-06

### Added
- Initial release
- Remove unsafe usage (#1)

[Unreleased]: https://github.com/cnpryer/httprs/compare/v0.0.1a2...HEAD
[0.0.1a2]: https://github.com/cnpryer/httprs/compare/v0.0.1a1...v0.0.1a2
[0.0.1a1]: https://github.com/cnpryer/httprs/compare/v0.0.1a0...v0.0.1a1
[0.0.1a0]: https://github.com/cnpryer/httprs/releases/tag/v0.0.1a0
