//! Copernicus DEM tile naming, download and on-disk cache.

use std::path::{Path, PathBuf};

use reqwest::blocking::Client;

use crate::error::{GeoError, Result};
use crate::geo::GeoBounds;

/// Root of the public Copernicus DEM GLO-30 bucket.
pub const COPERNICUS_GLO30_BASE: &str = "https://copernicus-dem-30m.s3.eu-central-1.amazonaws.com";

/// A DEM source that keeps whatever it has already downloaded.
///
/// Tiles are immutable and named after the degree cell they cover, so the cache
/// is a plain directory and a second run of the importer performs no request at
/// all.
pub struct DemSource {
    client: Client,
    base_url: String,
    cache_dir: PathBuf,
}

impl DemSource {
    /// Creates a Copernicus DEM GLO-30 source caching tiles under `cache_dir`.
    pub fn copernicus_glo30(cache_dir: impl Into<PathBuf>) -> Self {
        Self {
            client: Client::new(),
            base_url: COPERNICUS_GLO30_BASE.to_string(),
            cache_dir: cache_dir.into(),
        }
    }

    /// Replaces the bucket root, for a mirror or a local fixture server.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// Directory the downloaded tiles are kept in.
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// Name of the one-degree tile that contains a position.
    ///
    /// The product names each tile after the south-west corner of the degree
    /// cell it covers, so a position is floored rather than rounded: `29.99` and
    /// `29.01` both belong to `N29`.
    pub fn tile_name(lat: f64, lon: f64) -> String {
        let lat_index = lat.floor() as i64;
        let lon_index = lon.floor() as i64;
        let ns = if lat_index < 0 { 'S' } else { 'N' };
        let ew = if lon_index < 0 { 'W' } else { 'E' };
        format!(
            "Copernicus_DSM_COG_10_{ns}{:02}_00_{ew}{:03}_00_DEM",
            lat_index.abs(),
            lon_index.abs()
        )
    }

    /// Names of every tile a box touches.
    ///
    /// A box never spans more than two cells per axis, so this returns one, two
    /// or four names and always in a deterministic order.
    pub fn tiles_for(&self, bounds: &GeoBounds) -> Vec<String> {
        let mut names = Vec::new();
        let lat_rows = tile_indices(bounds.south, bounds.north);
        let lon_columns = tile_indices(bounds.west, bounds.east);
        for lat in &lat_rows {
            for lon in &lon_columns {
                names.push(Self::tile_name(*lat as f64, *lon as f64));
            }
        }
        names
    }

    /// Local path of a tile, whether or not it has been downloaded.
    pub fn tile_path(&self, name: &str) -> PathBuf {
        self.cache_dir.join(format!("{name}.tif"))
    }

    /// Ensures a tile is present locally, downloading it when missing.
    pub fn ensure_tile(&self, name: &str) -> Result<PathBuf> {
        let path = self.tile_path(name);
        if path.is_file() {
            tracing::debug!(
                tile = name,
                path = %path.display(),
                "using the cached dem tile"
            );
            return Ok(path);
        }
        std::fs::create_dir_all(&self.cache_dir)?;
        let url = format!(
            "{base}/{name}/{name}.tif",
            base = self.base_url,
            name = name
        );
        tracing::info!(tile = name, %url, "downloading dem tile");
        let mut response = self
            .client
            .get(&url)
            .send()
            .and_then(|response| response.error_for_status())
            .map_err(|source| GeoError::Http {
                url: url.clone(),
                source,
            })?;
        // A partial download must never be mistaken for a cached tile, so it is
        // written beside the target and only renamed once it is complete.
        let partial = path.with_extension("tif.part");
        let mut file = std::fs::File::create(&partial)?;
        let written = std::io::copy(&mut response, &mut file)?;
        file.sync_all()?;
        drop(file);
        if written == 0 {
            std::fs::remove_file(&partial)?;
            return Err(GeoError::data(format!("{url} returned an empty body")));
        }
        std::fs::rename(&partial, &path)?;
        tracing::info!(tile = name, bytes = written, "dem tile cached");
        Ok(path)
    }
}

/// Integer degree indices a span touches.
///
/// The upper bound is exclusive when it falls exactly on a degree boundary, so a
/// box that ends at `107.000` does not pull in the tile starting there.
fn tile_indices(min: f64, max: f64) -> Vec<i64> {
    let first = min.floor() as i64;
    let last = if max.fract() == 0.0 {
        max as i64 - 1
    } else {
        max.floor() as i64
    };
    (first..=last.max(first)).collect()
}
