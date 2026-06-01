# Changelog

## 0.2.1 - 2026-06-01

- Added GDAL VSI path normalization for common object-storage URLs:
  `s3://`, `gs://`, `az://`, `oss://`, and `http(s)://`.
- Reused the normalized VSI path for both GDAL dataset opening and direct
  TIFF structure reads, so remote object-storage validation follows the same
  code path as local validation.
- Exported `normalize_vsi_path` and `normalize_vsi_path_str` for callers that
  need to inspect or reuse the GDAL VSI path.
- Documented `/vsis3/` object-storage usage and GDAL credential configuration.
- Added two COG fixtures with non-ASCII object names to the local parity test
  corpus.

## 0.2.0 - 2026-06-01

- Matched validation behavior with GDAL's reference `rouault/cog_validator`.
- Added structured validation reports with warnings.
- Added configurable strictness options for layout, georeferencing, overviews,
  tile dimensions, compression, interleave, and tiling checks.
- Added direct VSI reads for TIFF structural metadata and block validation.
