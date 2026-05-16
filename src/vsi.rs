use gdal_sys::{VSIFCloseL, VSIFOpenL, VSIFReadL, VSIFSeekL, VSIVirtualHandle};
use std::{
    cell::Cell,
    ffi::{c_void, CStr, CString},
    path::Path,
    ptr,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum VSIError {
    #[error("Failed to seek file")]
    SeekError,
    #[error("Failed to open file")]
    OpenError,
    #[error("Failed to read expected number of bytes")]
    ReadError,
    #[error("Failed to close file")]
    CloseError,
    #[error("Path contains an interior nul byte")]
    PathContainsNul,
    #[error("Invalid whence value: {0}")]
    InvalidWhence(i32),
    #[error("File handle is already closed")]
    ClosedError,
}

#[derive(Debug, Clone, Copy)]
pub enum FileAccessMode {
    Read,
    ReadBinary,
    Write,
    WriteBinary,
    Append,
    AppendBinary,
    ReadWrite,
    ReadWriteBinary,
    WriteRead,
    WriteReadBinary,
    AppendRead,
    AppendReadBinary,
}

impl FileAccessMode {
    fn as_c_mode(&self) -> &'static CStr {
        match *self {
            FileAccessMode::Read => c"r",
            FileAccessMode::ReadBinary => c"rb",
            FileAccessMode::Write => c"w",
            FileAccessMode::WriteBinary => c"wb",
            FileAccessMode::Append => c"a",
            FileAccessMode::AppendBinary => c"ab",
            FileAccessMode::ReadWrite => c"r+",
            FileAccessMode::ReadWriteBinary => c"r+b",
            FileAccessMode::WriteRead => c"w+",
            FileAccessMode::WriteReadBinary => c"wb+",
            FileAccessMode::AppendRead => c"a+",
            FileAccessMode::AppendReadBinary => c"ab+",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Whence {
    SeekSet,
    SeekCur,
    SeekEnd,
}

impl TryFrom<i32> for Whence {
    type Error = VSIError;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Whence::SeekSet),
            1 => Ok(Whence::SeekCur),
            2 => Ok(Whence::SeekEnd),
            _ => Err(VSIError::InvalidWhence(value)),
        }
    }
}

impl From<Whence> for i32 {
    fn from(value: Whence) -> Self {
        match value {
            Whence::SeekSet => 0,
            Whence::SeekCur => 1,
            Whence::SeekEnd => 2,
        }
    }
}

/// Owning wrapper around a GDAL `VSIVirtualHandle`.
///
/// The handle is single-threaded: the type is intentionally `!Send + !Sync`
/// (it carries a raw pointer in a `Cell`). All FFI calls are scoped to the
/// owning thread.
pub struct VSIFile {
    c_vsilfile: Cell<*mut VSIVirtualHandle>,
}

impl VSIFile {
    pub fn vsi_fopenl(path: &Path, mode: FileAccessMode) -> Result<Self, VSIError> {
        let path_str = path.to_string_lossy();
        let filename_c =
            CString::new(path_str.as_ref()).map_err(|_| VSIError::PathContainsNul)?;
        let mode_c = mode.as_c_mode();
        // SAFETY: `filename_c` and `mode_c` are NUL-terminated CStrings whose
        // backing storage lives for the duration of this call. GDAL returns
        // a null pointer on failure, which we surface as `OpenError`.
        let file_handle = unsafe { VSIFOpenL(filename_c.as_ptr(), mode_c.as_ptr()) };
        if file_handle.is_null() {
            return Err(VSIError::OpenError);
        }
        Ok(Self {
            c_vsilfile: Cell::new(file_handle),
        })
    }

    fn open_handle(&self) -> Result<*mut VSIVirtualHandle, VSIError> {
        let handle = self.c_vsilfile.get();
        if handle.is_null() {
            return Err(VSIError::ClosedError);
        }
        Ok(handle)
    }

    pub fn vsi_fseekl(&self, offset: u64, whence: Whence) -> Result<(), VSIError> {
        let handle = self.open_handle()?;
        // SAFETY: `handle` is a live VSI handle obtained from `VSIFOpenL`.
        let rc = unsafe { VSIFSeekL(handle, offset, i32::from(whence)) };
        if rc != 0 {
            return Err(VSIError::SeekError);
        }
        Ok(())
    }

    pub fn vsi_freadl(&self, buffer: &mut [u8]) -> Result<(), VSIError> {
        let handle = self.open_handle()?;
        // SAFETY: `handle` is live; `buffer.as_mut_ptr()` is valid for
        // `buffer.len()` bytes for writes.
        let bytes_read = unsafe {
            VSIFReadL(
                buffer.as_mut_ptr().cast::<c_void>(),
                1,
                buffer.len(),
                handle,
            )
        };
        if bytes_read != buffer.len() {
            return Err(VSIError::ReadError);
        }
        Ok(())
    }

    /// Closes the handle. Idempotent: subsequent calls (including `Drop`) are
    /// no-ops. Even if GDAL reports a non-zero status, the handle is
    /// considered released to avoid a double-close in `Drop`.
    pub fn vsi_fclosel(&self) -> Result<(), VSIError> {
        let handle = self.c_vsilfile.replace(ptr::null_mut());
        if handle.is_null() {
            return Ok(());
        }
        // SAFETY: `handle` is a live VSI handle and we've already cleared the
        // cell so no one else (Drop, repeat close) can use it again.
        let rc = unsafe { VSIFCloseL(handle) };
        if rc != 0 {
            return Err(VSIError::CloseError);
        }
        Ok(())
    }

    pub fn read_exact_at(
        &self,
        buffer: &mut [u8],
        offset: u64,
        whence: Whence,
    ) -> Result<(), VSIError> {
        self.vsi_fseekl(offset, whence)?;
        self.vsi_freadl(buffer)
    }
}

impl Drop for VSIFile {
    fn drop(&mut self) {
        let handle = self.c_vsilfile.replace(ptr::null_mut());
        if !handle.is_null() {
            // SAFETY: `handle` was obtained from `VSIFOpenL` and has not been
            // closed yet (the cell was non-null).
            unsafe {
                VSIFCloseL(handle);
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_file_access_mode_to_c_str() {
        assert_eq!(FileAccessMode::Read.as_c_mode().to_str().unwrap(), "r");
        assert_eq!(
            FileAccessMode::ReadBinary.as_c_mode().to_str().unwrap(),
            "rb"
        );
        assert_eq!(FileAccessMode::Write.as_c_mode().to_str().unwrap(), "w");
        assert_eq!(
            FileAccessMode::WriteBinary.as_c_mode().to_str().unwrap(),
            "wb"
        );
    }

    #[test]
    fn test_whence_conversion() {
        assert_eq!(0, i32::from(Whence::SeekSet));
        assert_eq!(1, i32::from(Whence::SeekCur));
        assert_eq!(2, i32::from(Whence::SeekEnd));

        assert!(matches!(Whence::try_from(0), Ok(Whence::SeekSet)));
        assert!(matches!(Whence::try_from(1), Ok(Whence::SeekCur)));
        assert!(matches!(Whence::try_from(2), Ok(Whence::SeekEnd)));
    }

    #[test]
    fn test_whence_try_from_invalid_value() {
        let result = Whence::try_from(3);
        assert!(matches!(result, Err(VSIError::InvalidWhence(3))));
    }

    #[test]
    fn test_vsi_fopenl_rejects_path_with_nul() {
        let path = PathBuf::from("bad\0path");
        let result = VSIFile::vsi_fopenl(&path, FileAccessMode::ReadBinary);
        assert!(matches!(result, Err(VSIError::PathContainsNul)));
    }

    #[test]
    #[ignore = "requires external network access"]
    fn test_vsi_file_open_success() -> Result<(), VSIError> {
        let path =
            PathBuf::from("/vsicurl/https://download.osgeo.org/gdal/data/gtiff/small_world.tif");
        let vsi_file = VSIFile::vsi_fopenl(&path, FileAccessMode::ReadBinary)?;

        let mut buffer = [0u8; 2];
        vsi_file.read_exact_at(&mut buffer, 0, Whence::SeekSet)?;

        assert!(
            &buffer == b"II" || &buffer == b"MM",
            "Not a valid TIFF file header"
        );
        Ok(())
    }
}
