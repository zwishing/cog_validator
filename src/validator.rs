use crate::vsi::{FileAccessMode, VSIError, VSIFile, Whence};
use gdal::errors::GdalError;
use gdal::raster::RasterBand;
use gdal::{Dataset, Metadata};
use gdal_sys::CSLDestroy;
use std::ffi::CStr;
use std::fmt::Write as _;
use std::path::Path;
use thiserror::Error;

use byteorder::{ByteOrder, LittleEndian};

use libc::c_char;

const MAX_C_STRING_ARRAY_LEN: usize = 1_000_000;
const MAX_STRUCTURAL_MD_SIZE: usize = 1_000_000;
const REQUIRED_LAYOUT: &str = "COG";
const ALLOWED_COMPRESSIONS: [&str; 12] = [
    "LZW",
    "DEFLATE",
    "ZSTD",
    "LERC",
    "LERC_DEFLATE",
    "LERC_ZSTD",
    "WEBP",
    "JPEG",
    "YCbCr JPEG",
    "JXL",
    "PACKBITS",
    "CCITTFAX4",
];
const ALLOWED_INTERLEAVE: [&str; 3] = ["BAND", "PIXEL", "TILE"];

const STRUCTURAL_MD_PREFIX: &[u8] = b"GDAL_STRUCTURAL_METADATA_SIZE=";
const STRUCTURAL_MD_HEADER_LEN: usize = 43; // "GDAL_STRUCTURAL_METADATA_SIZE=NNNNNN bytes\n"

/// Validation knobs. Defaults match the `rouault/cog_validator` reference
/// (GDAL's Python validator): only the structural / spec-defined checks run
/// by default. Additional self-imposed strictness (LAYOUT=COG, georeferencing,
/// tile-dim alignment, etc.) is opt-in.
#[derive(Debug, Clone, Copy, Default)]
pub struct ValidationOptions {
    /// Require IMAGE_STRUCTURE/LAYOUT=COG. Not in the reference.
    pub require_cog_layout: bool,
    /// Require dataset projection + geotransform. Not in the reference.
    pub require_georeferencing: bool,
    /// Promote "large image without internal overviews" from warning to error.
    pub require_internal_overviews_for_large_images: bool,
    /// Require tile width/height to be multiples of 16. Not in the reference.
    pub require_tile_dimension_multiple_of_16: bool,
    /// Use stricter "is tiled" detection (any block dimension matching the
    /// band dimension counts as untiled). The reference only flags
    /// `block_width == XSize && block_width > 1024`.
    pub strict_tiled_detection: bool,
    /// Reject compressions outside the COG-recommended set. Not in reference.
    pub restrict_compression_to_cog_list: bool,
    /// Reject INTERLEAVE values outside `BAND` / `PIXEL` / `TILE`. Not in
    /// reference.
    pub restrict_interleave_to_cog_list: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ValidationReport {
    pub warnings: Vec<String>,
}

impl ValidationReport {
    fn warn(&mut self, message: impl Into<String>) {
        self.warnings.push(message.into());
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct StructuralMetadata {
    block_order_row_major: bool,
    block_leader_size_as_uint4: bool,
    block_trailer_last_4_bytes_repeated: bool,
    mask_interleaved_with_imagery: bool,
}

/// Errors emitted during Cloud Optimized GeoTIFF validation.
#[derive(Debug, Error)]
pub enum ValidateCOGError {
    #[error(transparent)]
    GdalError(#[from] GdalError),
    #[error("The file is not a GeoTIFF")]
    NotGeoTIFFError,
    #[error("Overviews found in external .ovr file. They should be internal")]
    ExternalOvrError,
    #[error("The file is greater than 512xH or Wx512, but is not tiled")]
    NotTiledError,
    #[error("invalid IMAGE_STRUCTURE metadata for {key}. expected '{expected}', found {found:?}")]
    InvalidImageStructureMetadata {
        key: String,
        expected: String,
        found: Option<String>,
    },
    #[error("unsupported compression '{compression}' for COG")]
    UnsupportedCompressionError { compression: String },
    #[error("missing required georeferencing metadata")]
    MissingGeoreferencingError,
    #[error(
        "{band_name} tile dimension {width}x{height} is invalid for COG (must be multiples of 16)"
    )]
    InvalidTileDimensions {
        band_name: String,
        width: usize,
        height: usize,
    },
    #[error("{band_name} is {width}x{height} but has no internal overviews")]
    MissingOverviewsError {
        band_name: String,
        width: usize,
        height: usize,
    },
    #[error("BLOCK_OFFSET_{x}_{y} metadata is missing")]
    MissingBlockOffsetMetadata { x: usize, y: usize },
    #[error("BLOCK_SIZE_{x}_{y} metadata is missing")]
    MissingBlockSizeMetadata { x: usize, y: usize },
    #[error("{band_name} block ({x}, {y}) offset is zero")]
    ZeroBlockOffsetError {
        band_name: String,
        x: usize,
        y: usize,
    },
    #[error("{band_name} block ({x}, {y}) offset is less than previous block.")]
    BlockOffsetError {
        band_name: String,
        x: usize,
        y: usize,
    },
    #[error("{band_name} block ({x}, {y}) starts at {offset}, overlapping previous block end {last_end}")]
    BlockDataOverlapError {
        band_name: String,
        x: usize,
        y: usize,
        last_end: u64,
        offset: u64,
    },
    #[error("{band_name} block ({x}, {y}) leader size ({leader_size}) does not match byte count ({byte_count}).")]
    LeaderSizeError {
        band_name: String,
        x: usize,
        y: usize,
        leader_size: u64,
        byte_count: u64,
    },
    #[error(transparent)]
    VSIError(#[from] VSIError),
    #[error("{band_name} block ({x},{y}) trailer bytes do not match.")]
    TrailerBytesError {
        band_name: String,
        x: usize,
        y: usize,
    },
    #[error("{key} has invalid value '{value}' at block ({x}, {y})")]
    InvalidMetadataValue {
        key: String,
        value: String,
        x: usize,
        y: usize,
    },
    #[error("null pointer returned for GDAL string array")]
    NullCStringArrayPointer,
    #[error("null pointer encountered in GDAL string array")]
    NullCStringPointer,
    #[error("offset arithmetic failed in {context}: offset={offset}, byte_count={byte_count}")]
    OffsetArithmeticError {
        context: &'static str,
        offset: u64,
        byte_count: u64,
    },
    #[error("GDAL string array exceeded maximum length without null terminator ({max_len})")]
    CStringArrayMissingTerminator { max_len: usize },
    #[error("invalid overview dimensions at level {level}: previous={prev_width}x{prev_height}, current={width}x{height}")]
    InvalidOverviewDimensions {
        level: usize,
        prev_width: usize,
        prev_height: usize,
        width: usize,
        height: usize,
    },
    #[error("invalid overview reduction factor at level {level}: prev=({prev_factor_x:.3},{prev_factor_y:.3}), current=({factor_x:.3},{factor_y:.3})")]
    InvalidOverviewReductionFactor {
        level: usize,
        prev_factor_x: f64,
        prev_factor_y: f64,
        factor_x: f64,
        factor_y: f64,
    },
    #[error("overview level {level} data offset {overview_offset} must be before main data offset {main_offset}")]
    InvalidOverviewDataOrdering {
        level: usize,
        overview_offset: u64,
        main_offset: u64,
    },
    #[error("KNOWN_INCOMPATIBLE_EDITION=YES is declared in the file")]
    KnownIncompatibleEdition,
    #[error("main IFD offset should be {expected} but is {found}")]
    InvalidMainIfdOffset { expected: u64, found: u64 },
    #[error("{band_name} is missing IFD_OFFSET metadata")]
    MissingIfdOffset { band_name: String },
    #[error("{band_name} has invalid IFD_OFFSET value '{value}'")]
    InvalidIfdOffsetValue { band_name: String, value: String },
    #[error(
        "overview {level} IFD offset {ifd_offset} should be greater than previous IFD offset {prev_ifd_offset}"
    )]
    InvalidOverviewIfdOrdering {
        level: usize,
        ifd_offset: u64,
        prev_ifd_offset: u64,
    },
    #[error("overview {level} is not tiled")]
    OverviewNotTiledError { level: usize },
    #[error("{band_name} first block offset {data_offset} is before its IFD offset {ifd_offset}")]
    FirstBlockBeforeIfd {
        band_name: String,
        data_offset: u64,
        ifd_offset: u64,
    },
    #[error(
        "main image first block offset {main_offset} should be after first block of largest overview {ovr_offset}"
    )]
    MainDataBeforeOverview { main_offset: u64, ovr_offset: u64 },
    #[error(
        "overview {level} first block offset {offset} should be after overview {next_level} first block offset {next_offset}"
    )]
    OverviewDataOrderingError {
        level: usize,
        next_level: usize,
        offset: u64,
        next_offset: u64,
    },
    #[error("{band_name} mask band block size differs from imagery band block size")]
    MaskBlockSizeMismatch { band_name: String },
    #[error(
        "{band_name} mask block ({x}, {y}) offset is {found}, expected {expected} (interleaved with imagery)"
    )]
    MaskOffsetMismatch {
        band_name: String,
        x: usize,
        y: usize,
        expected: u64,
        found: u64,
    },
    #[error("invalid GDAL_STRUCTURAL_METADATA block in file header")]
    InvalidStructuralMetadata,
}

/// Validates whether the file at `file_path` is a Cloud Optimized GeoTIFF.
///
/// Uses [`ValidationOptions::default`] which matches the behavior of
/// `rouault/cog_validator` (GDAL's reference Python validator).
pub fn validate_cloudgeotiff<P: AsRef<Path>>(file_path: &P) -> Result<bool, ValidateCOGError> {
    validate_cloudgeotiff_with_options(file_path, ValidationOptions::default())?;
    Ok(true)
}

pub fn validate_cloudgeotiff_with_options<P: AsRef<Path>>(
    file_path: &P,
    options: ValidationOptions,
) -> Result<ValidationReport, ValidateCOGError> {
    let dst = Dataset::open(file_path)?;
    if dst.driver().short_name() != "GTiff" {
        return Err(ValidateCOGError::NotGeoTIFFError);
    }

    let mut report = ValidationReport::default();
    validate_dataset(&dst, file_path.as_ref(), options, &mut report)?;
    Ok(report)
}

fn validate_dataset(
    dst: &Dataset,
    file_path: &Path,
    options: ValidationOptions,
    report: &mut ValidationReport,
) -> Result<(), ValidateCOGError> {
    check_image_structure_from_dataset(dst, &options)?;
    check_georeferencing(dst, &options)?;

    let file_list = collect_file_list(dst)?;
    check_external_ovr(&file_list)?;

    let band_count = dst.raster_count();
    if band_count == 0 {
        return Err(ValidateCOGError::InvalidImageStructureMetadata {
            key: "RASTER_COUNT".to_string(),
            expected: "at least one raster band".to_string(),
            found: Some("0".to_string()),
        });
    }

    let f = VSIFile::vsi_fopenl(file_path, FileAccessMode::ReadBinary)?;

    let main_band_1 = dst.rasterband(1)?;
    let main_ifd_offset = read_ifd_offset(&main_band_1, "main band")?;
    let structural_md = parse_structural_metadata(&f, main_ifd_offset)?;

    let ovr_count = main_band_1.overview_count()? as usize;
    check_dataset_has_overviews(&main_band_1, ovr_count, &options, report)?;
    check_overview_ifd_and_tiled(&main_band_1, ovr_count, main_ifd_offset)?;
    check_data_offsets_ordering(&main_band_1, ovr_count, main_ifd_offset)?;

    let mut key_buf = String::with_capacity(48);
    for band_idx in 1..=band_count {
        let band = dst.rasterband(band_idx)?;
        let band_name = format!("Band {}", band_idx);
        let band_ovr_count = band.overview_count()? as usize;

        check_main_band(&band_name, &band, &options)?;

        let interleaved_mask = if structural_md.mask_interleaved_with_imagery
            && band.mask_flags()?.is_per_dataset()
        {
            Some(band.open_mask_band()?)
        } else {
            None
        };
        validate_band(
            &f,
            &band_name,
            &band,
            &structural_md,
            interleaved_mask.as_ref(),
            &mut key_buf,
        )?;
        validate_mask_band(&f, &band_name, &band, &structural_md, &mut key_buf)?;
        validate_overviews(&f, &band, band_ovr_count, &band_name, &structural_md, &mut key_buf)?;
    }

    Ok(())
}

fn collect_file_list(dst: &Dataset) -> Result<Vec<String>, ValidateCOGError> {
    // SAFETY: `GDALGetFileList` returns a CSL (null-terminated array of
    // C strings) owned by GDAL; we hand it back via `CSLDestroy`. A null
    // return means no file list; both cases are handled.
    unsafe {
        let c_file_list = gdal_sys::GDALGetFileList(dst.c_dataset());
        if c_file_list.is_null() {
            return Ok(Vec::new());
        }
        let strings = string_array(c_file_list);
        CSLDestroy(c_file_list);
        strings
    }
}

fn check_image_structure_from_dataset(
    dst: &Dataset,
    options: &ValidationOptions,
) -> Result<(), ValidateCOGError> {
    let layout = dst.metadata_item("LAYOUT", "IMAGE_STRUCTURE");
    let compression = dst.metadata_item("COMPRESSION", "IMAGE_STRUCTURE");
    let interleave = dst.metadata_item("INTERLEAVE", "IMAGE_STRUCTURE");
    check_image_structure(
        layout.as_deref(),
        compression.as_deref(),
        interleave.as_deref(),
        options,
    )
}

fn check_georeferencing(
    dst: &Dataset,
    options: &ValidationOptions,
) -> Result<(), ValidateCOGError> {
    if !options.require_georeferencing {
        return Ok(());
    }
    if dst.projection().trim().is_empty() || dst.geo_transform().is_err() {
        return Err(ValidateCOGError::MissingGeoreferencingError);
    }
    Ok(())
}

fn check_external_ovr(file_list: &[String]) -> Result<(), ValidateCOGError> {
    for file in file_list {
        if Path::new(file)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("ovr"))
        {
            return Err(ValidateCOGError::ExternalOvrError);
        }
    }
    Ok(())
}

fn check_main_band(
    band_name: &str,
    band: &RasterBand,
    options: &ValidationOptions,
) -> Result<(), ValidateCOGError> {
    let block_size = band.block_size();
    if options.require_tile_dimension_multiple_of_16
        && (!block_size.0.is_multiple_of(16) || !block_size.1.is_multiple_of(16))
    {
        return Err(ValidateCOGError::InvalidTileDimensions {
            band_name: band_name.to_string(),
            width: block_size.0,
            height: block_size.1,
        });
    }

    if band.x_size() > 512 || band.y_size() > 512 {
        let strip_like = if options.strict_tiled_detection {
            block_size.0 == band.x_size() || block_size.1 == band.y_size()
        } else {
            block_size.0 == band.x_size() && block_size.0 > 1024
        };
        if strip_like {
            return Err(ValidateCOGError::NotTiledError);
        }
    }

    Ok(())
}

/// Dataset-level overview check (emitted once, not once per band).
fn check_dataset_has_overviews(
    main_band: &RasterBand,
    ovr_count: usize,
    options: &ValidationOptions,
    report: &mut ValidationReport,
) -> Result<(), ValidateCOGError> {
    if (main_band.x_size() > 512 || main_band.y_size() > 512) && ovr_count == 0 {
        if options.require_internal_overviews_for_large_images {
            return Err(ValidateCOGError::MissingOverviewsError {
                band_name: "main image".to_string(),
                width: main_band.x_size(),
                height: main_band.y_size(),
            });
        }
        report.warn(format!(
            "The file is {}x{} without internal overviews; including internal overviews is recommended",
            main_band.x_size(),
            main_band.y_size()
        ));
    }
    Ok(())
}

/// Reads the structural metadata block (if present) and validates that the
/// main IFD lives at byte 8 (classic TIFF), byte 16 (BigTIFF), or immediately
/// after the structural metadata block on a 2-byte boundary.
fn parse_structural_metadata(
    f: &VSIFile,
    main_ifd_offset: u64,
) -> Result<StructuralMetadata, ValidateCOGError> {
    let mut md = StructuralMetadata::default();

    let mut sig = [0u8; 4];
    f.read_exact_at(&mut sig, 0, Whence::SeekSet)?;
    let bigtiff = sig == [0x49, 0x49, 0x2B, 0x00] || sig == [0x4D, 0x4D, 0x00, 0x2B];
    let canonical_ifd_pos: u64 = if bigtiff { 16 } else { 8 };

    if main_ifd_offset == canonical_ifd_pos {
        return Ok(md);
    }

    let mut header = [0u8; STRUCTURAL_MD_HEADER_LEN];
    let mut expected_ifd_pos = canonical_ifd_pos;
    let read_ok = f
        .read_exact_at(&mut header, canonical_ifd_pos, Whence::SeekSet)
        .is_ok();

    if read_ok && header.starts_with(STRUCTURAL_MD_PREFIX) {
        let size_field = &header[STRUCTURAL_MD_PREFIX.len()..STRUCTURAL_MD_PREFIX.len() + 6];
        let size_str = std::str::from_utf8(size_field)
            .map_err(|_| ValidateCOGError::InvalidStructuralMetadata)?;
        let size: usize = size_str
            .trim()
            .parse()
            .map_err(|_| ValidateCOGError::InvalidStructuralMetadata)?;
        if size > MAX_STRUCTURAL_MD_SIZE {
            return Err(ValidateCOGError::InvalidStructuralMetadata);
        }

        let mut extra = vec![0u8; size];
        f.read_exact_at(
            &mut extra,
            canonical_ifd_pos + STRUCTURAL_MD_HEADER_LEN as u64,
            Whence::SeekSet,
        )?;

        md.block_order_row_major = byte_contains(&extra, b"BLOCK_ORDER=ROW_MAJOR");
        md.block_leader_size_as_uint4 = byte_contains(&extra, b"BLOCK_LEADER=SIZE_AS_UINT4");
        md.block_trailer_last_4_bytes_repeated =
            byte_contains(&extra, b"BLOCK_TRAILER=LAST_4_BYTES_REPEATED");
        md.mask_interleaved_with_imagery =
            byte_contains(&extra, b"MASK_INTERLEAVED_WITH_IMAGERY=YES");

        if byte_contains(&extra, b"KNOWN_INCOMPATIBLE_EDITION=YES") {
            return Err(ValidateCOGError::KnownIncompatibleEdition);
        }

        expected_ifd_pos += STRUCTURAL_MD_HEADER_LEN as u64 + size as u64;
        expected_ifd_pos += expected_ifd_pos % 2;
    }

    if expected_ifd_pos != main_ifd_offset {
        return Err(ValidateCOGError::InvalidMainIfdOffset {
            expected: expected_ifd_pos,
            found: main_ifd_offset,
        });
    }

    Ok(md)
}

fn byte_contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn read_ifd_offset(band: &RasterBand, band_name: &str) -> Result<u64, ValidateCOGError> {
    let value = band
        .metadata_item("IFD_OFFSET", "TIFF")
        .ok_or_else(|| ValidateCOGError::MissingIfdOffset {
            band_name: band_name.to_string(),
        })?;
    value
        .parse::<u64>()
        .map_err(|_| ValidateCOGError::InvalidIfdOffsetValue {
            band_name: band_name.to_string(),
            value,
        })
}

/// Validates that overview IFDs are sorted by increasing offset and that each
/// overview is itself tiled.
fn check_overview_ifd_and_tiled(
    main_band: &RasterBand,
    ovr_count: usize,
    main_ifd_offset: u64,
) -> Result<(), ValidateCOGError> {
    let mut prev_ifd_offset = main_ifd_offset;
    for level in 0..ovr_count {
        let ovr_band = main_band.overview(level)?;
        let ifd_offset = read_ifd_offset(&ovr_band, &format!("overview {level}"))?;
        if ifd_offset < prev_ifd_offset {
            return Err(ValidateCOGError::InvalidOverviewIfdOrdering {
                level,
                ifd_offset,
                prev_ifd_offset,
            });
        }

        let block_size = ovr_band.block_size();
        if block_size.0 == ovr_band.x_size() && block_size.0 > 1024 {
            return Err(ValidateCOGError::OverviewNotTiledError { level });
        }

        prev_ifd_offset = ifd_offset;
    }
    Ok(())
}

/// Validates the data-offset ordering across main image and overviews. COG
/// requires writing the smallest overview first and the main image last, so
/// first-block offsets must be monotonically decreasing as the level index
/// increases (level 0 is the largest overview, ovr_count-1 is the smallest).
fn check_data_offsets_ordering(
    main_band: &RasterBand,
    ovr_count: usize,
    main_ifd_offset: u64,
) -> Result<(), ValidateCOGError> {
    let main_data_offset = first_block_offset(main_band)?.unwrap_or(0);
    let mut overview_offsets: Vec<u64> = Vec::with_capacity(ovr_count);
    let mut overview_ifd_offsets: Vec<u64> = Vec::with_capacity(ovr_count);
    for level in 0..ovr_count {
        let ovr = main_band.overview(level)?;
        overview_offsets.push(first_block_offset(&ovr)?.unwrap_or(0));
        overview_ifd_offsets.push(read_ifd_offset(&ovr, &format!("overview {level}"))?);
    }

    let (last_data, last_ifd, last_name) = if ovr_count > 0 {
        let last = ovr_count - 1;
        (
            overview_offsets[last],
            overview_ifd_offsets[last],
            format!("overview {last}"),
        )
    } else {
        (main_data_offset, main_ifd_offset, "main image".to_string())
    };

    if last_data != 0 && last_data < last_ifd {
        return Err(ValidateCOGError::FirstBlockBeforeIfd {
            band_name: last_name,
            data_offset: last_data,
            ifd_offset: last_ifd,
        });
    }

    for i in (0..overview_offsets.len().saturating_sub(1)).rev() {
        let cur = overview_offsets[i];
        let next = overview_offsets[i + 1];
        if cur != 0 && cur < next {
            return Err(ValidateCOGError::OverviewDataOrderingError {
                level: i,
                next_level: i + 1,
                offset: cur,
                next_offset: next,
            });
        }
    }

    if let Some(&first_ovr) = overview_offsets.first() {
        if main_data_offset != 0 && main_data_offset < first_ovr {
            return Err(ValidateCOGError::MainDataBeforeOverview {
                main_offset: main_data_offset,
                ovr_offset: first_ovr,
            });
        }
    }

    Ok(())
}

/// Validates a band's blocks. Conditional checks (leader/trailer/row-major
/// ordering, mask interleave) are gated on the structural metadata flags.
///
/// `key_buf` is a caller-provided scratch buffer reused across all blocks of
/// all bands of the dataset to avoid per-block allocations.
fn validate_band(
    f: &VSIFile,
    band_name: &str,
    band: &RasterBand,
    md: &StructuralMetadata,
    interleaved_mask: Option<&RasterBand>,
    key_buf: &mut String,
) -> Result<(), ValidateCOGError> {
    let block_size = band.block_size();

    if let Some(mb) = interleaved_mask {
        if mb.block_size() != block_size {
            return Err(ValidateCOGError::MaskBlockSizeMismatch {
                band_name: band_name.to_string(),
            });
        }
    }

    let yblocks = band.y_size().div_ceil(block_size.1);
    let xblocks = band.x_size().div_ceil(block_size.0);
    let mut last_offset = 0_u64;
    let mut last_end = 0_u64;
    for y in 0..yblocks {
        for x in 0..xblocks {
            let (next_last_offset, next_last_end) = validate_block(
                f,
                band_name,
                band,
                x,
                y,
                last_offset,
                last_end,
                md,
                interleaved_mask,
                key_buf,
            )?;
            last_offset = next_last_offset;
            last_end = next_last_end;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn validate_block(
    f: &VSIFile,
    band_name: &str,
    band: &RasterBand,
    x: usize,
    y: usize,
    last_offset: u64,
    last_end: u64,
    md: &StructuralMetadata,
    interleaved_mask: Option<&RasterBand>,
    key_buf: &mut String,
) -> Result<(u64, u64), ValidateCOGError> {
    let offset = read_block_u64(band, key_buf, "BLOCK_OFFSET_", x, y, MetaKind::Offset)?;
    let byte_count = read_block_u64(band, key_buf, "BLOCK_SIZE_", x, y, MetaKind::Size)?;

    if offset == 0 {
        return Err(ValidateCOGError::ZeroBlockOffsetError {
            band_name: band_name.to_string(),
            x,
            y,
        });
    }

    if md.block_order_row_major && offset < last_offset {
        return Err(ValidateCOGError::BlockOffsetError {
            band_name: band_name.to_string(),
            x,
            y,
        });
    }

    if last_end > 0 && offset < last_end {
        return Err(ValidateCOGError::BlockDataOverlapError {
            band_name: band_name.to_string(),
            x,
            y,
            last_end,
            offset,
        });
    }

    if md.block_leader_size_as_uint4 && byte_count > 4 {
        check_leader_size(f, band_name, x, y, offset, byte_count)?;
    }
    if md.block_trailer_last_4_bytes_repeated {
        check_trailer_bytes(f, band_name, x, y, offset, byte_count)?;
    }

    if let Some(mb) = interleaved_mask {
        // Reuse the same "BLOCK_OFFSET_x_y" key by building it in-place again.
        let mask_offset =
            read_optional_block_u64(mb, key_buf, "BLOCK_OFFSET_", x, y)?.unwrap_or(0);
        if mask_offset > 0 {
            let leader_pad: u64 = if md.block_leader_size_as_uint4 { 4 } else { 0 };
            let trailer_pad: u64 = if md.block_trailer_last_4_bytes_repeated { 4 } else { 0 };
            let expected = offset
                .checked_add(byte_count)
                .and_then(|v| v.checked_add(leader_pad))
                .and_then(|v| v.checked_add(trailer_pad))
                .ok_or(ValidateCOGError::OffsetArithmeticError {
                    context: "mask_interleave_expected",
                    offset,
                    byte_count,
                })?;
            if mask_offset != expected {
                return Err(ValidateCOGError::MaskOffsetMismatch {
                    band_name: band_name.to_string(),
                    x,
                    y,
                    expected,
                    found: mask_offset,
                });
            }
        }
    }

    let next_last_end =
        offset
            .checked_add(byte_count)
            .ok_or(ValidateCOGError::OffsetArithmeticError {
                context: "block_end",
                offset,
                byte_count,
            })?;

    Ok((offset, next_last_end))
}

enum MetaKind {
    Offset,
    Size,
}

/// Reads a required `<prefix><x>_<y>` TIFF metadata item as `u64`, reusing
/// `key_buf` to avoid per-block allocations.
fn read_block_u64(
    band: &RasterBand,
    key_buf: &mut String,
    prefix: &str,
    x: usize,
    y: usize,
    kind: MetaKind,
) -> Result<u64, ValidateCOGError> {
    write_block_key(key_buf, prefix, x, y);
    let value = band.metadata_item(key_buf.as_str(), "TIFF").ok_or(match kind {
        MetaKind::Offset => ValidateCOGError::MissingBlockOffsetMetadata { x, y },
        MetaKind::Size => ValidateCOGError::MissingBlockSizeMetadata { x, y },
    })?;
    parse_metadata_u64(key_buf, &value, x, y)
}

/// Optional variant: returns `None` if the metadata item is absent.
fn read_optional_block_u64(
    band: &RasterBand,
    key_buf: &mut String,
    prefix: &str,
    x: usize,
    y: usize,
) -> Result<Option<u64>, ValidateCOGError> {
    write_block_key(key_buf, prefix, x, y);
    match band.metadata_item(key_buf.as_str(), "TIFF") {
        Some(value) => Ok(Some(parse_metadata_u64(key_buf, &value, x, y)?)),
        None => Ok(None),
    }
}

fn write_block_key(buf: &mut String, prefix: &str, x: usize, y: usize) {
    buf.clear();
    buf.push_str(prefix);
    let _ = write!(buf, "{x}_{y}");
}

fn check_leader_size(
    f: &VSIFile,
    band_name: &str,
    x: usize,
    y: usize,
    offset: u64,
    byte_count: u64,
) -> Result<(), ValidateCOGError> {
    if byte_count <= 4 {
        return Ok(());
    }
    let mut buf = [0u8; 4];
    let leader_offset = block_leader_offset(offset)?;
    f.read_exact_at(&mut buf, leader_offset, Whence::SeekSet)?;
    let leader_size = u64::from(LittleEndian::read_u32(&buf));
    if leader_size != byte_count {
        return Err(ValidateCOGError::LeaderSizeError {
            band_name: band_name.to_string(),
            x,
            y,
            leader_size,
            byte_count,
        });
    }
    Ok(())
}

fn check_trailer_bytes(
    f: &VSIFile,
    band_name: &str,
    x: usize,
    y: usize,
    offset: u64,
    byte_count: u64,
) -> Result<(), ValidateCOGError> {
    if byte_count < 4 {
        return Ok(());
    }
    let mut buf = [0u8; 8];
    let trailer_offset = block_trailer_offset(offset, byte_count)?;
    f.read_exact_at(&mut buf, trailer_offset, Whence::SeekSet)?;
    let (left, right) = buf.split_at(4);
    if left != right {
        return Err(ValidateCOGError::TrailerBytesError {
            band_name: band_name.to_string(),
            x,
            y,
        });
    }
    Ok(())
}

fn validate_mask_band(
    f: &VSIFile,
    band_name: &str,
    band: &RasterBand,
    md: &StructuralMetadata,
    key_buf: &mut String,
) -> Result<(), ValidateCOGError> {
    if band.mask_flags()?.is_per_dataset() {
        let mask_band = band.open_mask_band()?;
        validate_band(f, band_name, &mask_band, md, None, key_buf)?;
    }
    Ok(())
}

fn validate_overviews(
    f: &VSIFile,
    band: &RasterBand,
    ovr_count: usize,
    band_name: &str,
    md: &StructuralMetadata,
    key_buf: &mut String,
) -> Result<(), ValidateCOGError> {
    let mut overview_dimensions = Vec::with_capacity(ovr_count);
    for level in 0..ovr_count {
        let overview = band.overview(level)?;
        overview_dimensions.push((overview.x_size(), overview.y_size()));
    }
    check_overview_dimensions((band.x_size(), band.y_size()), &overview_dimensions)?;

    for level in 0..ovr_count {
        let ovr_band = band.overview(level)?;
        let ovr_name = format!("{} overview_{}", band_name, level);
        let interleaved_mask = if md.mask_interleaved_with_imagery
            && ovr_band.mask_flags()?.is_per_dataset()
        {
            Some(ovr_band.open_mask_band()?)
        } else {
            None
        };
        validate_band(
            f,
            &ovr_name,
            &ovr_band,
            md,
            interleaved_mask.as_ref(),
            key_buf,
        )?;
        validate_mask_band(f, &ovr_name, &ovr_band, md, key_buf)?;
    }
    Ok(())
}

fn first_block_offset(band: &RasterBand) -> Result<Option<u64>, ValidateCOGError> {
    let offset_key = "BLOCK_OFFSET_0_0";
    match band.metadata_item(offset_key, "TIFF") {
        Some(v) => Ok(Some(parse_metadata_u64(offset_key, &v, 0, 0)?)),
        None => Ok(None),
    }
}

fn check_image_structure(
    layout: Option<&str>,
    compression: Option<&str>,
    interleave: Option<&str>,
    options: &ValidationOptions,
) -> Result<(), ValidateCOGError> {
    if options.require_cog_layout
        && !layout.is_some_and(|v| v.eq_ignore_ascii_case(REQUIRED_LAYOUT))
    {
        return Err(ValidateCOGError::InvalidImageStructureMetadata {
            key: "LAYOUT".to_string(),
            expected: REQUIRED_LAYOUT.to_string(),
            found: layout.map(ToString::to_string),
        });
    }

    if options.restrict_compression_to_cog_list {
        let compression_value = compression.unwrap_or("NONE");
        if !ALLOWED_COMPRESSIONS
            .iter()
            .any(|v| v.eq_ignore_ascii_case(compression_value))
        {
            return Err(ValidateCOGError::UnsupportedCompressionError {
                compression: compression_value.to_string(),
            });
        }
    }

    if options.restrict_interleave_to_cog_list {
        let interleave_value = interleave.unwrap_or("UNKNOWN");
        if !ALLOWED_INTERLEAVE
            .iter()
            .any(|v| v.eq_ignore_ascii_case(interleave_value))
        {
            return Err(ValidateCOGError::InvalidImageStructureMetadata {
                key: "INTERLEAVE".to_string(),
                expected: ALLOWED_INTERLEAVE.join("|"),
                found: interleave.map(ToString::to_string),
            });
        }
    }

    Ok(())
}

fn check_overview_dimensions(
    main_dimensions: (usize, usize),
    overviews: &[(usize, usize)],
) -> Result<(), ValidateCOGError> {
    let mut prev_dimensions = main_dimensions;
    let mut prev_factor_x = 1.0_f64;
    let mut prev_factor_y = 1.0_f64;

    for (level, (width, height)) in overviews.iter().copied().enumerate() {
        if width == 0 || height == 0 || width >= prev_dimensions.0 || height >= prev_dimensions.1 {
            return Err(ValidateCOGError::InvalidOverviewDimensions {
                level,
                prev_width: prev_dimensions.0,
                prev_height: prev_dimensions.1,
                width,
                height,
            });
        }

        let factor_x = main_dimensions.0 as f64 / width as f64;
        let factor_y = main_dimensions.1 as f64 / height as f64;
        if factor_x <= prev_factor_x || factor_y <= prev_factor_y {
            return Err(ValidateCOGError::InvalidOverviewReductionFactor {
                level,
                prev_factor_x,
                prev_factor_y,
                factor_x,
                factor_y,
            });
        }

        prev_dimensions = (width, height);
        prev_factor_x = factor_x;
        prev_factor_y = factor_y;
    }

    Ok(())
}

// ---- Utility functions ----

fn parse_metadata_u64(key: &str, value: &str, x: usize, y: usize) -> Result<u64, ValidateCOGError> {
    value
        .parse::<u64>()
        .map_err(|_| ValidateCOGError::InvalidMetadataValue {
            key: key.to_string(),
            value: value.to_string(),
            x,
            y,
        })
}

fn block_leader_offset(offset: u64) -> Result<u64, ValidateCOGError> {
    offset
        .checked_sub(4)
        .ok_or(ValidateCOGError::OffsetArithmeticError {
            context: "leader",
            offset,
            byte_count: 0,
        })
}

fn block_trailer_offset(offset: u64, byte_count: u64) -> Result<u64, ValidateCOGError> {
    offset
        .checked_add(byte_count)
        .and_then(|v| v.checked_sub(4))
        .ok_or(ValidateCOGError::OffsetArithmeticError {
            context: "trailer",
            offset,
            byte_count,
        })
}

fn string_array(raw_ptr: *mut *mut c_char) -> Result<Vec<String>, ValidateCOGError> {
    convert_raw_ptr_array(raw_ptr, string)
}

fn string(raw_ptr: *const c_char) -> Result<String, ValidateCOGError> {
    if raw_ptr.is_null() {
        return Err(ValidateCOGError::NullCStringPointer);
    }
    // SAFETY: caller (GDAL CSL) guarantees `raw_ptr` points to a
    // NUL-terminated C string owned by GDAL for at least the duration of
    // this call.
    let c_str = unsafe { CStr::from_ptr(raw_ptr) };
    Ok(c_str.to_string_lossy().into_owned())
}

fn convert_raw_ptr_array<F, R>(
    raw_ptr: *mut *mut c_char,
    mut convert: F,
) -> Result<Vec<R>, ValidateCOGError>
where
    F: FnMut(*const c_char) -> Result<R, ValidateCOGError>,
{
    if raw_ptr.is_null() {
        return Err(ValidateCOGError::NullCStringArrayPointer);
    }

    let mut ret_val = Vec::new();
    for i in 0..MAX_C_STRING_ARRAY_LEN {
        // SAFETY: GDAL CSLs are null-terminated arrays of valid C string
        // pointers. We stop at the first null, and `MAX_C_STRING_ARRAY_LEN`
        // bounds the walk so a missing terminator can't cause OOB.
        let next = unsafe { raw_ptr.add(i).read() };
        if next.is_null() {
            return Ok(ret_val);
        }

        let value = convert(next)?;
        ret_val.push(value);
    }

    Err(ValidateCOGError::CStringArrayMissingTerminator {
        max_len: MAX_C_STRING_ARRAY_LEN,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_metadata_u64_returns_error_on_invalid_value() {
        let err = parse_metadata_u64("BLOCK_OFFSET_0_0", "abc", 0, 0).unwrap_err();
        assert!(matches!(
            err,
            ValidateCOGError::InvalidMetadataValue {
                key,
                value,
                x: 0,
                y: 0
            } if key == "BLOCK_OFFSET_0_0" && value == "abc"
        ));
    }

    #[test]
    fn checked_block_offsets_reject_underflow_and_overflow() {
        let underflow_err = block_leader_offset(2).unwrap_err();
        assert!(matches!(
            underflow_err,
            ValidateCOGError::OffsetArithmeticError { .. }
        ));

        let overflow_err = block_trailer_offset(u64::MAX, 10).unwrap_err();
        assert!(matches!(
            overflow_err,
            ValidateCOGError::OffsetArithmeticError { .. }
        ));
    }

    #[test]
    fn string_array_rejects_null_pointer() {
        let err = string_array(std::ptr::null_mut()).unwrap_err();
        assert!(matches!(err, ValidateCOGError::NullCStringArrayPointer));
    }

    #[test]
    fn check_image_structure_rejects_missing_layout_when_required() {
        let opts = ValidationOptions {
            require_cog_layout: true,
            ..ValidationOptions::default()
        };
        let err = check_image_structure(None, Some("LZW"), Some("BAND"), &opts).unwrap_err();
        assert!(matches!(
            err,
            ValidateCOGError::InvalidImageStructureMetadata { key, .. } if key == "LAYOUT"
        ));
    }

    #[test]
    fn check_image_structure_rejects_uncompressed_when_compression_restricted() {
        let opts = ValidationOptions {
            restrict_compression_to_cog_list: true,
            ..ValidationOptions::default()
        };
        let err =
            check_image_structure(Some("COG"), Some("NONE"), Some("BAND"), &opts).unwrap_err();
        assert!(matches!(
            err,
            ValidateCOGError::UnsupportedCompressionError { compression }
            if compression == "NONE"
        ));
    }

    #[test]
    fn check_image_structure_silent_by_default_on_missing_layout() {
        let result = check_image_structure(
            None,
            Some("LZW"),
            Some("BAND"),
            &ValidationOptions::default(),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn check_overview_dimensions_rejects_non_decreasing_level() {
        let err =
            check_overview_dimensions((4096, 4096), &[(2048, 2048), (2048, 1024)]).unwrap_err();
        assert!(matches!(
            err,
            ValidateCOGError::InvalidOverviewDimensions { level: 1, .. }
        ));
    }

    #[test]
    fn write_block_key_reuses_buffer() {
        let mut buf = String::new();
        write_block_key(&mut buf, "BLOCK_OFFSET_", 12, 34);
        assert_eq!(buf, "BLOCK_OFFSET_12_34");
        let cap_before = buf.capacity();
        write_block_key(&mut buf, "BLOCK_SIZE_", 5, 6);
        assert_eq!(buf, "BLOCK_SIZE_5_6");
        assert_eq!(buf.capacity(), cap_before);
    }

    #[test]
    fn byte_contains_finds_substring() {
        assert!(byte_contains(b"foo BLOCK_ORDER=ROW_MAJOR bar", b"BLOCK_ORDER=ROW_MAJOR"));
        assert!(!byte_contains(b"foo", b"BLOCK_ORDER=ROW_MAJOR"));
        assert!(byte_contains(b"abc", b""));
    }
}
