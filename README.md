# Cloud Optimized GeoTIFF Validator in Rust

## Introduction

This project is a Cloud Optimized GeoTIFF (COG) validator implemented in Rust using GDAL. It checks compliance of GeoTIFF files with the COG specification, ensuring they are optimized for cloud storage and efficient access.

Inspired by the [cog-validator-java](https://github.com/batugane/cog-validator-java) project, which provides a similar COG validator implemented in Java.

## Features

- **Supports GDAL Virtual File Systems**: Utilizes GDAL's Virtual File System capabilities for versatile data access.

## Requirements

- Rust Programming Language
- [GDAL](https://gdal.org/) Library (must be installed separately)

## Installation

To use the COG validator, clone the repository and compile the Rust code:

```bash
git clone https://github.com/Zwishing/cog_validator.git
cd cog_validator
cargo build --release
```

## Usage

```rust
use cog_validator::cog_validator;

fn main() {
    let result = cog_validator("/vsicurl/https://oin-hotosm.s3.amazonaws.com/59c66c5223c8440011d7b1e4/0/7ad397c0-bba2-4f98-a08a-931ec3a6e943.tif");
    println!("COG validation result: {:?}", result);
}
```

## License

This project is licensed under the MIT License. See the [LICENSE](LICENSE) file for details.


## Acknowledgments

- [cog-validator-java](https://github.com/batugane/cog-validator-java) for the initial implementation in Java.
- [GDAL](https://gdal.org/) for the powerful geospatial data handling capabilities. 



    