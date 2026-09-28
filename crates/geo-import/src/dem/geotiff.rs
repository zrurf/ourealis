//! Single-band GeoTIFF reader for the DEM tiles.
//!
//! The importer needs three things from a tile — the sample matrix, the pixel
//! size and the position of one pixel — which the GeoTIFF tags carry directly.
//! Only those are decoded: the projection tags and the colour model of a general
//! GeoTIFF are deliberately ignored, so an unusual file is rejected with a clear
//! message rather than silently mis-georeferenced.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use tiff::decoder::{Decoder, DecodingResult};
use tiff::tags::Tag;

use crate::error::{GeoError, Result};

/// GDAL's no-data tag; the TIFF crate has no named constant for it.
const GDAL_NODATA_TAG: u16 = 42113;

/// A decoded elevation raster with its georeferencing.
///
/// Rows run north to south, so `step_lat` is negative: the sample at `(x, y)` is
/// the elevation at `origin + (x * step_lon, y * step_lat)`.
#[derive(Debug, Clone)]
pub struct DemRaster {
    /// Number of columns.
    pub width: u32,
    /// Number of rows.
    pub height: u32,
    /// Longitude of the centre of pixel `(0, 0)`.
    pub origin_lon: f64,
    /// Latitude of the centre of pixel `(0, 0)`.
    pub origin_lat: f64,
    /// Longitude covered by one pixel, degrees.
    pub step_lon: f64,
    /// Latitude covered by one pixel, degrees, negative.
    pub step_lat: f64,
    /// Value the file marks as "no data", when it declares one.
    pub nodata: Option<f64>,
    /// Elevations in metres, row-major `[row][column]`.
    pub samples: Vec<f32>,
}

impl DemRaster {
    /// Decodes a GeoTIFF tile.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let file = File::open(path)?;
        let mut decoder = Decoder::new(BufReader::new(file)).map_err(|source| GeoError::Tiff {
            path: path.to_path_buf(),
            source,
        })?;
        let (width, height) = decoder.dimensions().map_err(|source| GeoError::Tiff {
            path: path.to_path_buf(),
            source,
        })?;
        if width < 2 || height < 2 {
            return Err(GeoError::data(format!(
                "{} declares a {width}x{height} raster, too small to interpolate",
                path.display()
            )));
        }

        let scale = decoder.get_tag_f64_vec(Tag::ModelPixelScaleTag).ok();
        let tiepoint = decoder.get_tag_f64_vec(Tag::ModelTiepointTag).ok();
        let nodata = decoder
            .get_tag_ascii_string(Tag::Unknown(GDAL_NODATA_TAG))
            .ok()
            .and_then(|text| {
                text.trim()
                    .trim_end_matches('\0')
                    .trim()
                    .parse::<f64>()
                    .ok()
            });
        let (step_lon, step_lat) = pixel_size(path, scale.as_deref())?;
        let (origin_lon, origin_lat) = pixel_origin(path, tiepoint.as_deref(), step_lon, step_lat)?;

        let image = decoder.read_image().map_err(|source| GeoError::Tiff {
            path: path.to_path_buf(),
            source,
        })?;
        let samples = to_f32(image).ok_or_else(|| {
            GeoError::data(format!(
                "{} does not store a scalar sample band the importer can read",
                path.display()
            ))
        })?;
        if samples.len() != width as usize * height as usize {
            return Err(GeoError::data(format!(
                "{} decoded {} sample(s) for a {width}x{height} raster",
                path.display(),
                samples.len()
            )));
        }
        Ok(Self {
            width,
            height,
            origin_lon,
            origin_lat,
            step_lon,
            step_lat,
            nodata,
            samples,
        })
    }

    /// Builds a raster from decoded parts.
    ///
    /// For callers that decode the sample band themselves; the same constraints
    /// the reader enforces apply, so a raster that cannot be sampled is refused
    /// here rather than producing a silently misplaced grid.
    pub fn from_parts(
        width: u32,
        height: u32,
        origin_lon: f64,
        origin_lat: f64,
        step_lon: f64,
        step_lat: f64,
        nodata: Option<f64>,
        samples: Vec<f32>,
    ) -> Result<Self> {
        if width < 2 || height < 2 {
            return Err(GeoError::data(format!(
                "a {width}x{height} raster is too small to interpolate"
            )));
        }
        if step_lon <= 0.0 || step_lat >= 0.0 {
            return Err(GeoError::data(format!(
                "a pixel size of {step_lon} x {step_lat} degrees is not a north-up geographic grid"
            )));
        }
        if samples.len() != width as usize * height as usize {
            return Err(GeoError::data(format!(
                "{} sample(s) do not fill a {width}x{height} raster",
                samples.len()
            )));
        }
        Ok(Self {
            width,
            height,
            origin_lon,
            origin_lat,
            step_lon,
            step_lat,
            nodata,
            samples,
        })
    }

    /// Fractional pixel coordinates of a geographic position.
    fn pixel_of(&self, lon: f64, lat: f64) -> (f64, f64) {
        (
            (lon - self.origin_lon) / self.step_lon,
            (lat - self.origin_lat) / self.step_lat,
        )
    }

    /// Whether a position falls inside the sampled footprint.
    pub fn covers(&self, lon: f64, lat: f64) -> bool {
        let (x, y) = self.pixel_of(lon, lat);
        x >= 0.0 && y >= 0.0 && x <= (self.width - 1) as f64 && y <= (self.height - 1) as f64
    }

    /// Bilinearly samples the raster.
    ///
    /// Returns `None` outside the footprint and on any pixel of the 2x2
    /// neighbourhood that carries no data, so a void is never averaged into a
    /// plausible-looking height.
    pub fn sample(&self, lon: f64, lat: f64) -> Option<f64> {
        let (fx, fy) = self.pixel_of(lon, lat);
        if !self.covers(lon, lat) {
            return None;
        }
        // The footprint check admits the last row and column, where the upper
        // neighbour of the interpolation window does not exist; those positions
        // fall back to the edge pixel rather than being reported as a void.
        let x0 = (fx.floor() as i64).clamp(0, self.width as i64 - 2);
        let y0 = (fy.floor() as i64).clamp(0, self.height as i64 - 2);
        let (tx, ty) = (fx - x0 as f64, fy - y0 as f64);
        let (a, b) = (self.value(x0, y0)?, self.value(x0 + 1, y0)?);
        let (c, d) = (self.value(x0, y0 + 1)?, self.value(x0 + 1, y0 + 1)?);
        Some(a * (1.0 - tx) * (1.0 - ty) + b * tx * (1.0 - ty) + c * (1.0 - tx) * ty + d * tx * ty)
    }

    /// One sample, or `None` when it is out of range, non-finite or no-data.
    fn value(&self, x: i64, y: i64) -> Option<f64> {
        if x < 0 || y < 0 || x >= self.width as i64 || y >= self.height as i64 {
            return None;
        }
        let value = self.samples[y as usize * self.width as usize + x as usize] as f64;
        if !value.is_finite() {
            return None;
        }
        match self.nodata {
            Some(nodata) if (value - nodata).abs() <= f64::EPSILON * nodata.abs().max(1.0) => None,
            _ => Some(value),
        }
    }
}

/// Pixel size from the `ModelPixelScaleTag`, latitude made negative.
fn pixel_size(path: &Path, scale: Option<&[f64]>) -> Result<(f64, f64)> {
    let scale = scale.and_then(|values| values.get(..2)).ok_or_else(|| {
        GeoError::data(format!(
            "{} carries no ModelPixelScaleTag, so its pixel size is unknown",
            path.display()
        ))
    })?;
    let (step_lon, step_lat) = (scale[0], -scale[1]);
    if step_lon <= 0.0 || step_lat >= 0.0 {
        return Err(GeoError::data(format!(
            "{} declares a pixel size of {step_lon} x {step_lat} degrees, which is not a north-up geographic grid",
            path.display()
        )));
    }
    Ok((step_lon, step_lat))
}

/// Centre of pixel `(0, 0)` from the `ModelTiepointTag`.
///
/// A tiepoint maps a *raster* point `(i, j)` onto a model point `(x, y)`, and by
/// convention that point is the corner of the pixel rather than its centre, so
/// half a pixel is added back. Pixel centres are what sampling works in, and
/// getting this wrong would shift the whole map by half a cell.
fn pixel_origin(
    path: &Path,
    tiepoint: Option<&[f64]>,
    step_lon: f64,
    step_lat: f64,
) -> Result<(f64, f64)> {
    let ties = tiepoint.filter(|values| values.len() >= 5).ok_or_else(|| {
        GeoError::data(format!(
            "{} carries no ModelTiepointTag, so its position on the globe is unknown",
            path.display()
        ))
    })?;
    let (i, j, x, y) = (ties[0], ties[1], ties[3], ties[4]);
    Ok((x + (0.5 - i) * step_lon, y + (0.5 - j) * step_lat))
}

/// Converts a single-band decoding result into `f32` samples.
fn to_f32(image: DecodingResult) -> Option<Vec<f32>> {
    Some(match image {
        DecodingResult::F32(values) => values,
        DecodingResult::F64(values) => values.into_iter().map(|value| value as f32).collect(),
        DecodingResult::I8(values) => values.into_iter().map(f32::from).collect(),
        DecodingResult::I16(values) => values.into_iter().map(f32::from).collect(),
        DecodingResult::I32(values) => values.into_iter().map(|value| value as f32).collect(),
        DecodingResult::I64(values) => values.into_iter().map(|value| value as f32).collect(),
        DecodingResult::U8(values) => values.into_iter().map(f32::from).collect(),
        DecodingResult::U16(values) => values.into_iter().map(f32::from).collect(),
        DecodingResult::U32(values) => values.into_iter().map(|value| value as f32).collect(),
        DecodingResult::U64(values) => values.into_iter().map(|value| value as f32).collect(),
        DecodingResult::F16(_) => return None,
    })
}
