use crate::vsi::{FileAccessMode, VSIError, VSIFile, Whence};
use gdal::raster::RasterBand;
use gdal_sys::CSLDestroy;
use std::ffi::CStr;
use std::path::Path;

use gdal::errors::GdalError;
use gdal::{Dataset, Metadata};
use thiserror::Error;

use byteorder::{ByteOrder, LittleEndian};
use std::str;

use libc::c_char;

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
    #[error("BLOCK_OFFSET_{x}_{y} is empty")]
    EmptyOffsetError { x: usize, y: usize },
    #[error("{band_name} block ({x}, {y}) offset is less than previous block.")]
    BlockOffsetError {
        band_name: String,
        x: usize,
        y: usize,
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
}

pub fn validate_cloudgeotiff<P: AsRef<Path>>(file_path: &P) -> Result<bool, ValidateCOGError> {
    let dst = &Dataset::open(file_path)?;
    if dst.driver().short_name() != "GTiff" {
        return Err(ValidateCOGError::NotGeoTIFFError);
    };
    _validate(dst, file_path.as_ref())?;
    Ok(true)
}

fn _validate(dst: &Dataset, file_path: &Path) -> Result<bool, ValidateCOGError> {
    let main_band = &dst.rasterband(1)?;
    let ovr_count = main_band.overview_count()?;

    let file_list = unsafe {
        let c_file_list = gdal_sys::GDALGetFileList(dst.c_dataset());
        let strings = _string_array(c_file_list);
        CSLDestroy(c_file_list);
        strings
    };

    _check_main_band(main_band, ovr_count)?;
    _check_external_ovr(file_list)?;
    let f = &VSIFile::vsi_fopenl(file_path, FileAccessMode::ReadBinary)?;
    _validate_band(f, "Main resolution image", main_band)?;
    _validate_mask_band(f, "Main resolution image", main_band)?;
    _validate_ovr(f, main_band, ovr_count)?;
    f.vsi_fclosel()?;
    Ok(true)
}

fn _check_external_ovr(file_list: Vec<String>) -> Result<bool, ValidateCOGError> {
    if !file_list.is_empty() {
        for file in file_list {
            if file.ends_with(".ovr") {
                return Err(ValidateCOGError::ExternalOvrError);
            }
        }
    }
    Ok(true)
}

fn _check_main_band(band: &RasterBand, ovr_count: i32) -> Result<bool, ValidateCOGError> {
    if band.x_size() > 512 || band.y_size() > 512 {
        let block_size = band.block_size();
        if block_size.0 == band.x_size() && block_size.0 > 1024 {
            return Err(ValidateCOGError::NotTiledError);
        }
        if ovr_count == 0 {
            // warning：
            // The file is greater than 512xH or Wx512, it is recommended
            // to include internal overviews"
            println!("Warning: The file is greater than 512xH or Wx512, it is recommended to include internal overviews");
        }
    }
    Ok(true)
}

fn _validate_band(
    f: &VSIFile,
    band_name: &str,
    band: &RasterBand,
) -> Result<bool, ValidateCOGError> {
    let block_size = band.block_size();
    let yblocks = (band.y_size() + block_size.1 - 1) / block_size.1;
    let xblocks = (band.x_size() + block_size.0 - 1) / block_size.0;
    let last_offset = 0_u64;
    for y in 0..yblocks {
        for x in 0..xblocks {
            _validate_block(f, band_name, band, x, y, last_offset)?;
        }
    }
    Ok(true)
}

fn _validate_block(
    f: &VSIFile,
    band_name: &str,
    band: &RasterBand,
    x: usize,
    y: usize,
    last_offset: u64,
) -> Result<bool, ValidateCOGError> {
    let offset = match band.metadata_item(format!("BLOCK_OFFSET_{x}_{y}").as_str(), "TIFF") {
        Some(i) => i.parse::<u64>().unwrap_or(0),
        None => return Err(ValidateCOGError::EmptyOffsetError { x, y }),
    };
    let byte_count = match band.metadata_item(format!("BLOCK_SIZE_{x}_{y}").as_str(), "TIFF") {
        Some(i) => i.parse::<u64>().unwrap_or(0),
        None => return Err(ValidateCOGError::EmptyOffsetError { x, y }),
    };
    if offset > 0 {
        if offset < last_offset {
            return Err(ValidateCOGError::BlockOffsetError {
                band_name: band_name.to_string(),
                x,
                y,
            });
        };
        _check_leader_size(f, band_name, x, y, offset, byte_count)?;
        _check_trailer_bytes(f, band_name, x, y, offset, byte_count)?;
    };
    Ok(true)
}

fn _check_leader_size(
    f: &VSIFile,
    band_name: &str,
    x: usize,
    y: usize,
    offset: u64,
    byte_count: u64,
) -> Result<bool, ValidateCOGError> {
    if byte_count > 4 {
        let mut buf = [0u8; 4];
        f.read_exact_at(&mut buf, offset - 4, Whence::SeekSet)?;
        let leader_size = LittleEndian::read_u32(&buf) as u64;
        if leader_size != byte_count {
            return Err(ValidateCOGError::LeaderSizeError {
                band_name: band_name.to_string(),
                x,
                y,
                leader_size,
                byte_count,
            });
        }
    }
    Ok(true)
}

fn _check_trailer_bytes(
    f: &VSIFile,
    band_name: &str,
    x: usize,
    y: usize,
    offset: u64,
    byte_count: u64,
) -> Result<bool, ValidateCOGError> {
    if byte_count >= 4 {
        let mut buf = [0u8; 8];
        f.read_exact_at(&mut buf, offset + byte_count - 4, Whence::SeekSet)?;
        let (left, right) = buf.split_at(4);
        if left != right {
            return Err(ValidateCOGError::TrailerBytesError {
                band_name: band_name.to_string(),
                x,
                y,
            });
        }
    }
    Ok(true)
}

fn _validate_mask_band(
    f: &VSIFile,
    band_name: &str,
    band: &RasterBand,
) -> Result<bool, ValidateCOGError> {
    if band.mask_flags()?.is_per_dataset() {
        let mask_band = &band.open_mask_band()?;
        _validate_band(f, band_name, mask_band)?;
    }
    Ok(true)
}

fn _validate_ovr(f: &VSIFile, band: &RasterBand, ovr_count: i32) -> Result<bool, ValidateCOGError> {
    for i in 0..ovr_count {
        let ovr_band = &band.overview(i as usize)?;
        let ovr = format!("overview_{}", i);
        _validate_band(f, ovr.as_str(), ovr_band)?;
        _validate_mask_band(f, ovr.as_str(), ovr_band)?;
    }
    Ok(true)
}

// util functions from gdal
pub fn _string_array(raw_ptr: *mut *mut c_char) -> Vec<String> {
    _convert_raw_ptr_array(raw_ptr, _string)
}
pub fn _string(raw_ptr: *const c_char) -> String {
    let c_str = unsafe { CStr::from_ptr(raw_ptr) };
    c_str.to_string_lossy().into_owned()
}
fn _convert_raw_ptr_array<F, R>(raw_ptr: *mut *mut c_char, convert: F) -> Vec<R>
where
    F: Fn(*const c_char) -> R,
{
    let mut ret_val = Vec::new();
    let mut i = 0;
    unsafe {
        loop {
            let ptr = raw_ptr.add(i);
            if ptr.is_null() {
                break;
            }
            let next = ptr.read();
            if next.is_null() {
                break;
            }
            let value = convert(next);
            i += 1;
            ret_val.push(value);
        }
    }
    ret_val
}
