use std::path::Path;

pub mod validator;
pub mod vsi;

pub fn cog_validator<P: AsRef<Path>>(path: P) -> Result<bool, validator::ValidateCOGError> {
    validator::validate_cloudgeotiff(&path)
}

#[cfg(test)]
mod tests {
    use std::env;

    use super::*;
    #[test]
    pub fn test_cog_validator_from_http() {
        let url = "/vsicurl/https://oin-hotosm.s3.amazonaws.com/59c66c5223c8440011d7b1e4/0/7ad397c0-bba2-4f98-a08a-931ec3a6e943.tif";
        let result = cog_validator(url);
        assert_eq!(result.is_err(), true)
    }

    #[test]
    pub fn test_cog_validator_from_local() {
        let mut current_dir = env::current_dir().unwrap();
        current_dir.push("src/data/PuertoRicoTropicalFruit_cog.tif");
        let result = cog_validator(current_dir).unwrap();
        assert_eq!(result, true)
        
    }
}
