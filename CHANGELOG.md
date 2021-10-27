# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Added
- Added versioning code from operator-rs for up and downgrades ([#55]).
- Added `ProductVersion` to status ([#55]).
- Use sticky scheduler ([#67])

### Changed:
- `operator-rs` `0.2.2` → `0.3.0` ([#85]).
- Moved `wait_until_crds_present` to `operator-binary` ([#85]).

### Removed
- Dependency `kube-rs` ([#85]).
- Dependency `k8s-openapi` ([#85]).
- Dependency `product-config` ([#85]).
- Code for version handling ([#55]).
- Removed `current_version` and `target_version` from cluster status ([#55]).

[#85]: https://github.com/stackabletech/monitoring-operator/pull/85
[#67]: https://github.com/stackabletech/monitoring-operator/pull/67
[#61]: https://github.com/stackabletech/monitoring-operator/pull/61
[#55]: https://github.com/stackabletech/monitoring-operator/pull/55

## [0.2.0] - 2021-09-14

### Changed
- **Breaking:** Repository structure was changed and the -server crate renamed to -binary. As part of this change the -server suffix was removed from both the package name for os packages and the name of the executable ([#41]).

## [0.1.0] - 2021.09.07

### Added

- Initial release
