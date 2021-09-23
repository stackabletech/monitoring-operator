# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Added
- Added versioning code from operator-rs for up and downgrades ([#55]).
- Added `ProductVersion` to status ([#55]).

### Changed:
- `kube-rs`: `0.59` → `0.60` ([#61]).
- `k8s-openapi` `0.12` → `0.13` and features: `v1_21` → `v1_22` ([#61]).
- `operator-rs` `0.2.1` → `0.2.2` ([#61]).

### Removed
- Code for version handling ([#55]).
- Removed `current_version` and `target_version` from cluster status ([#55]).

[#61]: https://github.com/stackabletech/monitoring-operator/pull/61
[#55]: https://github.com/stackabletech/monitoring-operator/pull/55

## [0.2.0] - 2021-09-14

### Changed
- **Breaking:** Repository structure was changed and the -server crate renamed to -binary. As part of this change the -server suffix was removed from both the package name for os packages and the name of the executable ([#41]).

## [0.1.0] - 2021.09.07

### Added

- Initial release
