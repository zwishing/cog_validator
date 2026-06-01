# Cloud Optimized GeoTIFF Validator in Rust

## Introduction

This project is a Cloud Optimized GeoTIFF (COG) validator implemented in Rust using GDAL. It checks compliance of GeoTIFF files with the COG specification, ensuring they are optimized for cloud storage and efficient access.

The validation logic mirrors [rouault/cog_validator](https://github.com/rouault/cog_validator) (the reference Python validator shipped with GDAL) and was initially inspired by [cog-validator-java](https://github.com/batugane/cog-validator-java).

## Features

- **Supports GDAL Virtual File Systems**: Validates local files and remote sources via `/vsicurl/`, `/vsis3/`, `/vsimem/`, etc. Common object-storage URLs such as `s3://bucket/key.tif` and `https://...` are normalized to GDAL VSI paths automatically.
- **Warnings vs errors**: Hard errors fail validation; soft issues (e.g. large image without overviews) are returned as warnings via `ValidationReport`.
- **Configurable strictness** through `ValidationOptions`.

## Validation Checks

### File-level structure
- Driver must be `GTiff`.
- Overviews must be internal — no external `.ovr` sidecar allowed.
- Main IFD must be at offset `8` (classic TIFF) / `16` (BigTIFF), or immediately after a `GDAL_STRUCTURAL_METADATA` block aligned to a 2-byte boundary.
- `GDAL_STRUCTURAL_METADATA` block is parsed for the flags:
  - `BLOCK_ORDER=ROW_MAJOR`
  - `BLOCK_LEADER=SIZE_AS_UINT4`
  - `BLOCK_TRAILER=LAST_4_BYTES_REPEATED`
  - `MASK_INTERLEAVED_WITH_IMAGERY=YES`
- `KNOWN_INCOMPATIBLE_EDITION=YES` is rejected.

### Image structure metadata
- `LAYOUT=COG` is required (configurable, downgradable to warning).
- `COMPRESSION` must be one of: `LZW`, `DEFLATE`, `ZSTD`, `LERC`, `LERC_DEFLATE`, `LERC_ZSTD`, `WEBP`, `JPEG`, `JXL`, `PACKBITS`, `CCITTFAX4`.
- `INTERLEAVE` must be `BAND`, `PIXEL`, or `TILE`.
- Georeferencing (projection + geotransform) is required (configurable).

### Main image / bands
- Images larger than 512 px in either dimension must be tiled.
- Tile dimensions must be multiples of 16.
- Large images without internal overviews produce a warning (configurable to error).

### Overviews
- Overview dimensions must strictly decrease as the level index increases.
- Overview reduction factor (in both x and y) must strictly increase as the level index increases.
- Overview IFD offsets must be in increasing order.
- Each overview must be tiled.
- First data block of the smallest overview must be after its own IFD.
- For multi-overview files: data block of overview `i` must be after data block of overview `i+1` (smallest overview is written first).
- First data block of the main image must be after the first data block of overview 0 (largest overview).

### Per-block (when corresponding structural metadata flags are present)
- `BLOCK_LEADER=SIZE_AS_UINT4`: the uint32 leader preceding each block must match its byte count.
- `BLOCK_TRAILER=LAST_4_BYTES_REPEATED`: the last 4 bytes of each block must be repeated as the trailing 4 bytes.
- `BLOCK_ORDER=ROW_MAJOR`: block offsets within a band must be non-decreasing in row-major order.

### Mask bands
- Per-dataset mask bands are recursively validated.
- When `MASK_INTERLEAVED_WITH_IMAGERY=YES`:
  - Mask block size must match the imagery band block size.
  - For each block, `mask_offset == imagery_offset + byte_count + leader_pad + trailer_pad`.

## Requirements

- Rust toolchain
- [GDAL](https://gdal.org/) library (installed separately)

## Installation

```bash
git clone https://github.com/Zwishing/cog_validator.git
cd cog_validator
cargo build --release
```

## Usage

### Basic check (boolean result)

```rust
use cog_validator::cog_validator;

fn main() {
    let result = cog_validator("/vsicurl/https://oin-hotosm.s3.amazonaws.com/59c66c5223c8440011d7b1e4/0/7ad397c0-bba2-4f98-a08a-931ec3a6e943.tif");
    println!("COG validation result: {:?}", result);
}
```

### Detailed report with warnings

```rust
use cog_validator::{cog_validator_with_options, validator::ValidationOptions};

fn main() {
    let report = cog_validator_with_options(
        "path/to/file.tif",
        ValidationOptions::default(),
    );
    match report {
        Ok(r) => {
            for w in &r.warnings {
                println!("warning: {w}");
            }
            println!("valid COG with {} warning(s)", r.warnings.len());
        }
        Err(e) => eprintln!("invalid COG: {e}"),
    }
}
```

### Object storage via GDAL VSI

```rust
use cog_validator::cog_validator_with_options;
use cog_validator::validator::ValidationOptions;

fn main() {
    let report = cog_validator_with_options(
        "s3://my-bucket/path/to/cog.tif",
        ValidationOptions::default(),
    );
    println!("{report:?}");
}
```

The validator also accepts explicit GDAL paths such as `/vsis3/bucket/key.tif` and `/vsicurl/https://host/key.tif`. Credentials and endpoint configuration are handled by GDAL.

For authenticated `/vsis3/` reads, configure GDAL before calling the validator:

```rust
use cog_validator::{cog_validator_with_options, ValidationOptions};
use gdal::config::set_config_option;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    set_config_option("AWS_ACCESS_KEY_ID", "your-appkey")?;
    set_config_option("AWS_SECRET_ACCESS_KEY", "your-secret")?;
    set_config_option("AWS_REGION", "us-east-1")?;

    let report = cog_validator_with_options(
        "s3://my-bucket/path/to/cog.tif",
        ValidationOptions::default(),
    )?;
    println!("{report:?}");
    Ok(())
}
```

For temporary credentials, also set `AWS_SESSION_TOKEN`. For S3-compatible
services, set provider-specific GDAL options such as `AWS_S3_ENDPOINT`,
`AWS_HTTPS`, and `AWS_VIRTUAL_HOSTING`.

### Configuring strictness

```rust
use cog_validator::validator::ValidationOptions;

let options = ValidationOptions {
    require_cog_layout: true,                          // require LAYOUT=COG
    require_georeferencing: false,                     // downgrade to warning
    require_internal_overviews_for_large_images: true, // promote to error
};
```

| Option | Default | Effect |
|---|---|---|
| `require_cog_layout` | `true` | Missing `LAYOUT=COG` is an error (otherwise warning). |
| `require_georeferencing` | `true` | Missing projection or geotransform is an error (otherwise warning). |
| `require_internal_overviews_for_large_images` | `false` | When `true`, large image without overviews is an error instead of a warning. |

## License

Licensed under the Apache License 2.0. See [LICENSE](LICENSE) for details.

## Acknowledgments

- [rouault/cog_validator](https://github.com/rouault/cog_validator) — the reference Python validator whose check semantics this project mirrors.
- [cog-validator-java](https://github.com/batugane/cog-validator-java) — the original inspiration.
- [GDAL](https://gdal.org/) — geospatial data handling library this validator builds on.
