use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TakeoutJsonMetadata {
    pub title: Option<String>,
    pub description: Option<String>,
    #[serde(rename = "photoTakenTime")]
    pub photo_taken_time: Option<TimestampInfo>,
    #[serde(rename = "creationTime")]
    pub creation_time: Option<TimestampInfo>,
    #[serde(rename = "geoData")]
    pub geo_data: Option<GeoDataInfo>,
    #[serde(rename = "geoDataExif")]
    pub geo_data_exif: Option<GeoDataInfo>,
    pub people: Option<Vec<PersonInfo>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimestampInfo {
    pub timestamp: String,
    pub formatted: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeoDataInfo {
    pub latitude: f64,
    pub longitude: f64,
    pub altitude: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonInfo {
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct MediaFilePair {
    pub media_path: std::path::PathBuf,
    #[allow(dead_code)]
    pub sidecar_path: Option<std::path::PathBuf>,
    pub album_name: Option<String>,
    pub is_edited_version: bool,
    pub metadata: Option<TakeoutJsonMetadata>,
}
