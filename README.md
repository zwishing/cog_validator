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
