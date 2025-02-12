use gdal_sys::{VSIFCloseL, VSIFOpenL, VSIFReadL, VSIFSeekL, VSIVirtualHandle};
use std::{ffi::{c_void, CString}, path::Path};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum VSIError {
    #[error("Failed to seek file")]
    SeekError,    // Error when seeking within a file fails
    #[error("Failed to open file")]
    OpenError,    // Error when opening a file fails
    #[error("Failed to read expected number of bytes")]
    ReadError,    // Error when reading the expected number of bytes fails
    #[error("Failed to close file")]
    CloseError,   // Error when closing a file fails
}

#[derive(Debug)]
pub enum FileAccessMode {
    Read,           // Open file for reading only
    ReadBinary,     // Open file for reading in binary mode
    Write,          // Open file for writing only
    WriteBinary,    // Open file for writing in binary mode
    Append,         // Open file for appending
    AppendBinary,   // Open file for appending in binary mode
    ReadWrite,      // Open file for both reading and writing
    ReadWriteBinary, // Open file for both reading and writing in binary mode
    WriteRead,      // Open file for writing and reading
    WriteReadBinary, // Open file for writing and reading in binary mode
    AppendRead,     // Open file for appending and reading
    AppendReadBinary, // Open file for appending and reading in binary mode
}

impl FileAccessMode {
    fn to_c_str(&self) -> CString {
        match *self {
            FileAccessMode::Read => CString::new("r").expect("CString::new failed for Read"),
            FileAccessMode::ReadBinary => {
                CString::new("rb").expect("CString::new failed for ReadBinary")
            }
            FileAccessMode::Write => CString::new("w").expect("CString::new failed for Write"),
            FileAccessMode::WriteBinary => {
                CString::new("wb").expect("CString::new failed for WriteBinary")
            }
            FileAccessMode::Append => CString::new("a").expect("CString::new failed for Append"),
            FileAccessMode::AppendBinary => {
                CString::new("ab").expect("CString::new failed for AppendBinary")
            }
            FileAccessMode::ReadWrite => {
                CString::new("r+").expect("CString::new failed for ReadWrite")
            }
            FileAccessMode::ReadWriteBinary => {
                CString::new("r+b").expect("CString::new failed for ReadWriteBinary")
            }
            FileAccessMode::WriteRead => {
                CString::new("w+").expect("CString::new failed for WriteRead")
            }
            FileAccessMode::WriteReadBinary => {
                CString::new("wb+").expect("CString::new failed for WriteReadBinary")
            }
            FileAccessMode::AppendRead => {
                CString::new("a+").expect("CString::new failed for AppendRead")
            }
            FileAccessMode::AppendReadBinary => {
                CString::new("ab+").expect("CString::new failed for AppendReadBinary")
            }
        }
    }
}

pub enum Whence {
    SeekSet,    // Seek from the beginning of the file
    SeekCur,    // Seek from the current position
    SeekEnd,    // Seek from the end of the file
}

impl From<i32> for Whence {
    fn from(value: i32) -> Self {
        match value {
            0 => Whence::SeekSet,
            1 => Whence::SeekCur,
            2 => Whence::SeekEnd,
            _ => panic!("Invalid whence value"),
        }
    }
}

impl Into<i32> for Whence {
    fn into(self) -> i32 {
        match self {
            Whence::SeekSet => 0,
            Whence::SeekCur => 1,
            Whence::SeekEnd => 2,
        }
    }
}

pub struct VSIFile {
    c_vsilfile: *mut VSIVirtualHandle,  // Raw pointer to GDAL's virtual file handle
}

impl VSIFile {
    /// Opens a file using GDAL's Virtual File System
    /// 
    /// # Arguments
    /// * `path` - Path to the file to open
    /// * `mode` - File access mode
    pub fn vsi_fopenl(path: &Path, mode: FileAccessMode) -> Result<Self, VSIError> {
        unsafe {
            let path_str = path.to_string_lossy();
            let filename_c = CString::new(path_str.as_ref()).expect("CString conversion failed");
            let mode_c = mode.to_c_str();
            let file_handle = VSIFOpenL(filename_c.as_ptr(), mode_c.as_ptr());
            if file_handle.is_null() {
                return Err(VSIError::OpenError);
            }
            Ok(Self {
                c_vsilfile: file_handle,
            })
        }
    }

    /// Seeks to a position in the file
    /// 
    /// # Arguments
    /// * `offset` - Number of bytes to offset from the whence position
    /// * `whence` - Position from where to seek
    pub fn vsi_fseekl(&self, offset: u64, whence: Whence) -> Result<(), VSIError> {
        let n = unsafe { VSIFSeekL(self.c_vsilfile(), offset, whence.into()) };
        if n != 0 {
            self.vsi_fclosel()?;
            return Err(VSIError::SeekError);
        };
        Ok(())
    }

    /// Reads data from the file into a buffer
    /// 
    /// # Arguments
    /// * `buffer` - Buffer to read the data into
    pub fn vsi_freadl(&self, buffer: &mut [u8]) -> Result<usize, VSIError> {
        let bytes_read = unsafe {
            VSIFReadL(
                buffer.as_mut_ptr() as *mut c_void,
                // The size of each data block, in bytes. It is typically the size of the data type being read.
                // For example, if reading data of type `i32`, this value should be `sizeof(i32)`.
                1,
                buffer.len(),
                self.c_vsilfile(),
            )
        };
        if bytes_read != buffer.len() {
            return Err(VSIError::ReadError);
        }
        Ok(bytes_read)
    }

    /// Closes the file
    pub fn vsi_fclosel(&self) -> Result<(), VSIError> {
        unsafe {
            if VSIFCloseL(self.c_vsilfile()) != 0 {
                return Err(VSIError::CloseError);
            }
        };
        Ok(())
    }

    /// Reads exact number of bytes at a specific position in the file
    /// 
    /// # Arguments
    /// * `buffer` - Buffer to read the data into
    /// * `offset` - Position to start reading from
    /// * `whenc` - Position reference for the offset
    pub fn read_exact_at(
        &self,
        buffer: &mut [u8],
        offset: u64,
        whenc: Whence,
    ) -> Result<usize, VSIError> {
        self.vsi_fseekl(offset, whenc)?;
        let n = self.vsi_freadl(buffer)?;
        Ok(n)
    }

    /// Returns the raw GDAL virtual file handle
    pub fn c_vsilfile(&self) -> *mut VSIVirtualHandle {
        self.c_vsilfile
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_file_access_mode_to_c_str() {
        assert_eq!(FileAccessMode::Read.to_c_str().to_str().unwrap(), "r");
        assert_eq!(FileAccessMode::ReadBinary.to_c_str().to_str().unwrap(), "rb");
        assert_eq!(FileAccessMode::Write.to_c_str().to_str().unwrap(), "w");
        assert_eq!(FileAccessMode::WriteBinary.to_c_str().to_str().unwrap(), "wb");
    }

    #[test]
    fn test_whence_conversion() {
        assert_eq!(0, Whence::SeekSet.into());
        assert_eq!(1, Whence::SeekCur.into());
        assert_eq!(2, Whence::SeekEnd.into());

        assert!(matches!(Whence::from(0), Whence::SeekSet));
        assert!(matches!(Whence::from(1), Whence::SeekCur));
        assert!(matches!(Whence::from(2), Whence::SeekEnd));
    }


    #[test]
    fn test_vsi_file_open_success() -> Result<(), VSIError> {
        let path = PathBuf::from("/vsicurl/https://download.osgeo.org/gdal/data/gtiff/small_world.tif");
        let vsi_file = VSIFile::vsi_fopenl(&path, FileAccessMode::ReadBinary)?;
        
        // Verify if the file is opened successfully
        let mut buffer = [0u8; 2];
        vsi_file.read_exact_at(&mut buffer, 0, Whence::SeekSet)?;
        
        // Check TIFF file header magic number
        assert!(
            &buffer == b"II" || &buffer == b"MM",
            "Not a valid TIFF file header"
        );

        vsi_file.vsi_fclosel()?;
        Ok(())
    }
}
