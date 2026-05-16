use std::path::Path;

pub mod validator;
pub mod vsi;

pub use validator::{ValidateCOGError, ValidationOptions, ValidationReport};

pub fn cog_validator<P: AsRef<Path>>(path: P) -> Result<bool, validator::ValidateCOGError> {
    validator::validate_cloudgeotiff(&path)
}

pub fn cog_validator_with_options<P: AsRef<Path>>(
    path: P,
    options: validator::ValidationOptions,
) -> Result<validator::ValidationReport, validator::ValidateCOGError> {
    validator::validate_cloudgeotiff_with_options(&path, options)
}

#[cfg(test)]
mod tests {
    //! End-to-end tests against the bundled COG corpus under `data/`.
    //!
    //! Expected behavior mirrors `rouault/cog_validator` (the GDAL reference
    //! Python validator). Files in the `VALID` set must validate without
    //! warnings. Files in `VALID_WITH_NO_OVERVIEW_WARNING` are valid but
    //! produce the "large image without internal overviews" warning. Files
    //! in `LERC_REQUIRES_CODEC` are valid COGs but their LERC codec is not
    //! compiled into this build of GDAL.
    use super::*;
    use crate::validator::{ValidateCOGError, ValidationOptions};
    use std::path::PathBuf;

    const VALID: &[&str] = &[
        "68077a72c46a9912474701ef.tif",
        "AOT.tif",
        "PuertoRicoTropicalFruit.tiff",
        "PuertoRicoTropicalFruit_cog.tif",
        "antimeridian.tif",
        "cog_rgb_with_stats.tif",
        "cog_uint8_rgb_mask.tif",
        "cog_uint8_rgb_nodata.tif",
        "cog_uint8_rgba.tif",
        "eox_cloudless.tif",
        "maxar_opendata_yellowstone_visual.tif",
        "sydney_airport_GEC.tif",
        "xjejfvrbm1fbu1ecw-0000000000-0000008192.tif",
    ];

    const VALID_WITH_NO_OVERVIEW_WARNING: &[&str] =
        &["beach-geotiff.tif", "office-geotiff.tif"];

    const LERC_REQUIRES_CODEC: &[&str] = &[
        "float32_1band_lerc_block32.tif",
        "float32_1band_lerc_zstd_block32.tif",
    ];

    fn data_path(name: &str) -> PathBuf {
        let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.push("fixtures");
        p.push(name);
        p
    }

    #[test]
    fn parity_with_reference_valid_files() {
        for name in VALID {
            let report =
                cog_validator_with_options(data_path(name), ValidationOptions::default())
                    .unwrap_or_else(|e| panic!("{name} expected valid, got error: {e}"));
            assert!(
                report.warnings.is_empty(),
                "{name} expected no warnings, got: {:?}",
                report.warnings
            );
        }
    }

    #[test]
    fn parity_with_reference_no_overview_warning() {
        for name in VALID_WITH_NO_OVERVIEW_WARNING {
            let report =
                cog_validator_with_options(data_path(name), ValidationOptions::default())
                    .unwrap_or_else(|e| panic!("{name} expected valid, got error: {e}"));
            assert!(
                report
                    .warnings
                    .iter()
                    .any(|w| w.contains("internal overviews")),
                "{name} expected an overview warning, got: {:?}",
                report.warnings
            );
        }
    }

    #[test]
    #[ignore = "requires GDAL with LERC codec compiled in"]
    fn lerc_files_require_codec() {
        for name in LERC_REQUIRES_CODEC {
            cog_validator_with_options(data_path(name), ValidationOptions::default())
                .unwrap_or_else(|e| panic!("{name} expected valid (LERC build), got: {e}"));
        }
    }

    #[test]
    fn strict_layout_can_be_enforced_via_options() {
        // With strict options, files without LAYOUT=COG must hard-fail.
        let strict = ValidationOptions {
            require_cog_layout: true,
            ..ValidationOptions::default()
        };
        let err = cog_validator_with_options(data_path("beach-geotiff.tif"), strict)
            .expect_err("expected LAYOUT error under strict options");
        assert!(
            matches!(
                err,
                ValidateCOGError::InvalidImageStructureMetadata { ref key, .. } if key == "LAYOUT"
            ),
            "expected LAYOUT InvalidImageStructureMetadata, got: {err}"
        );
    }

    #[test]
    fn strict_overview_requirement_promotes_warning_to_error() {
        let strict = ValidationOptions {
            require_internal_overviews_for_large_images: true,
            ..ValidationOptions::default()
        };
        let err = cog_validator_with_options(data_path("beach-geotiff.tif"), strict)
            .expect_err("expected MissingOverviewsError under strict options");
        assert!(
            matches!(err, ValidateCOGError::MissingOverviewsError { .. }),
            "expected MissingOverviewsError, got: {err}"
        );
    }

    #[test]
    fn basic_api_returns_true_for_valid_cog() {
        let result = cog_validator(data_path("PuertoRicoTropicalFruit_cog.tif")).unwrap();
        assert!(result);
    }

    #[test]
    #[ignore = "requires external network access"]
    fn cog_validator_from_http() {
        let url = "/vsicurl/https://oin-hotosm.s3.amazonaws.com/59c66c5223c8440011d7b1e4/0/7ad397c0-bba2-4f98-a08a-931ec3a6e943.tif";
        let _ = cog_validator(url);
    }
}
