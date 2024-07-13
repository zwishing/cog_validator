use gdal_sys::{VSIFCloseL, VSIFOpenL, VSIFReadL, VSIFSeekL, VSIVirtualHandle};
use std::ffi::{c_void, CString};
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
}

#[derive(Debug)]
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
    SeekSet,
    SeekCur,
    SeekEnd,
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
    c_vsilfile: *mut VSIVirtualHandle,
}

impl VSIFile {
    pub fn vsi_fopenl(url: &str, mode: FileAccessMode) -> Result<Self, VSIError> {
        unsafe {
            let filename_c = CString::new(url).expect("CString conversion failed");
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

    pub fn vsi_fseekl(&self, offset: u64, whence: Whence) -> Result<(), VSIError> {
        let n = unsafe { VSIFSeekL(self.c_vsilfile(), offset, whence.into()) };
        if n != 0 {
            self.vsi_fclosel()?;
            return Err(VSIError::SeekError);
        };
        Ok(())
    }

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

    pub fn vsi_fclosel(&self) -> Result<(), VSIError> {
        unsafe {
            if VSIFCloseL(self.c_vsilfile()) != 0 {
                return Err(VSIError::CloseError);
            }
        };
        Ok(())
    }

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

    pub fn c_vsilfile(&self) -> *mut VSIVirtualHandle {
        self.c_vsilfile
    }
}

#[cfg(test)]
mod test {
    use super::VSIFile;
}
