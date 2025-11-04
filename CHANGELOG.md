# Changelog

## [2.0] - 2025-11-04
---

### Changed
- Only count files that are determined to contain source code (based on file name/extension heuristics).
 Can be turned off with `--all-fileytpes` flag. Since the new behavior defaults to being on, this is a breaking change that requires bumping version numbesr.

## [1.0] - 2025-10-07
---
Initial release

### Added
- Scan repos and create a cohorts.json file with "yearly cohort" information
- Plot cohorts.json using uv
