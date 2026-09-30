use std::path::Path;
use std::process::Command;
use chrono::{DateTime, Utc};
use tracing::{debug, info, warn};
use crate::takeout::models::TakeoutJsonMetadata;

pub struct MetadataEngine {
    has_exiftool: bool,
}

impl MetadataEngine {
    pub fn new() -> Self {
        let has_exiftool = Command::new("exiftool")
            .arg("-ver")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        if has_exiftool {
            info!("ExifTool detected on system. Will leverage ExifTool for video and HEIC metadata.");
        } else {
            debug!("ExifTool not found. Running with pure Rust metadata engine.");
        }

        Self { has_exiftool }
    }

    pub fn format_description(meta: &TakeoutJsonMetadata) -> Option<String> {
        let original_desc = meta.description.as_deref().unwrap_or("").trim();
        let people_names: Vec<String> = meta
            .people
            .as_ref()
            .map(|list| {
                list.iter()
                    .map(|p| p.name.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        if people_names.is_empty() {
            if original_desc.is_empty() {
                None
            } else {
                Some(original_desc.to_string())
            }
        } else {
            let people_str = format!("People: {}", people_names.join(", "));
            if original_desc.is_empty() {
                Some(people_str)
            } else {
                Some(format!("{}\n\n{}", original_desc, people_str))
            }
        }
    }

    pub fn format_timestamp(timestamp_str: &str) -> Option<String> {
        let secs: i64 = timestamp_str.parse().ok()?;
        let dt = DateTime::<Utc>::from_timestamp(secs, 0)?;
        Some(dt.format("%Y:%m:%d %H:%M:%S").to_string())
    }

    pub fn embed_metadata(&self, file_path: &Path, meta: &TakeoutJsonMetadata) -> Result<(), String> {
        let ext = file_path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();

        // If ExifTool is available, use it for best fidelity across all formats
        if self.has_exiftool {
            if let Err(e) = self.embed_with_exiftool(file_path, meta) {
                warn!("ExifTool failed on {:?}: {}. Falling back to pure Rust engine.", file_path, e);
            } else {
                return Ok(());
            }
        }

        // Pure Rust engine fallback
        match ext.as_str() {
            "jpg" | "jpeg" => self.embed_jpeg_pure_rust(file_path, meta),
            "png" => self.embed_png_pure_rust(file_path, meta),
            "webp" => self.embed_webp_pure_rust(file_path, meta),
            _ => {
                debug!("Skipping pure Rust metadata injection for format '.{}' on {:?}", ext, file_path);
                Ok(())
            }
        }
    }

    fn embed_with_exiftool(&self, file_path: &Path, meta: &TakeoutJsonMetadata) -> Result<(), String> {
        let mut cmd = Command::new("exiftool");
        cmd.arg("-overwrite_original");

        // Timestamp
        let ts = meta
            .photo_taken_time
            .as_ref()
            .map(|t| &t.timestamp)
            .or_else(|| meta.creation_time.as_ref().map(|t| &t.timestamp));

        if let Some(ts_str) = ts {
            if let Some(formatted_time) = Self::format_timestamp(ts_str) {
                cmd.arg(format!("-AllDates={}", formatted_time));
                cmd.arg(format!("-QuickTime:CreateDate={}", formatted_time));
            }
        }

        // GPS Coordinates
        let geo = meta.geo_data.as_ref().or(meta.geo_data_exif.as_ref());
        if let Some(g) = geo {
            if g.latitude != 0.0 || g.longitude != 0.0 {
                cmd.arg(format!("-GPSLatitude={}", g.latitude.abs()));
                cmd.arg(format!(
                    "-GPSLatitudeRef={}",
                    if g.latitude >= 0.0 { "N" } else { "S" }
                ));
                cmd.arg(format!("-GPSLongitude={}", g.longitude.abs()));
                cmd.arg(format!(
                    "-GPSLongitudeRef={}",
                    if g.longitude >= 0.0 { "E" } else { "W" }
                ));
                cmd.arg(format!("-GPSAltitude={}", g.altitude));
            }
        }

        // Description and People
        if let Some(desc) = Self::format_description(meta) {
            cmd.arg(format!("-ImageDescription={}", desc));
            cmd.arg(format!("-Description={}", desc));
            cmd.arg(format!("-Caption-Abstract={}", desc));
        }

        // People / Known Faces tags into XMP and IPTC
        if let Some(people) = &meta.people {
            for p in people {
                let name = p.name.trim();
                if !name.is_empty() {
                    cmd.arg(format!("-XMP-iptcExt:PersonInImage+={}", name));
                    cmd.arg(format!("-Subject+={}", name));
                    cmd.arg(format!("-Keywords+={}", name));
                }
            }
        }

        cmd.arg(file_path);

        let output = cmd
            .output()
            .map_err(|e| format!("Failed to execute exiftool: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("ExifTool exited with error: {}", stderr));
        }

        Ok(())
    }

    fn embed_jpeg_pure_rust(&self, file_path: &Path, meta: &TakeoutJsonMetadata) -> Result<(), String> {
        let mut metadata = little_exif::metadata::Metadata::new();

        let ts = meta
            .photo_taken_time
            .as_ref()
            .map(|t| &t.timestamp)
            .or_else(|| meta.creation_time.as_ref().map(|t| &t.timestamp));

        if let Some(ts_str) = ts {
            if let Some(formatted_time) = Self::format_timestamp(ts_str) {
                metadata.set_tag(little_exif::exif_tag::ExifTag::DateTimeOriginal(formatted_time.clone()));
                metadata.set_tag(little_exif::exif_tag::ExifTag::CreateDate(formatted_time.clone()));
                metadata.set_tag(little_exif::exif_tag::ExifTag::ModifyDate(formatted_time));
            }
        }

        if let Some(desc) = Self::format_description(meta) {
            metadata.set_tag(little_exif::exif_tag::ExifTag::ImageDescription(desc));
        }

        // Write to file
        metadata.write_to_file(file_path).map_err(|e| format!("Failed to write EXIF to JPEG {:?}: {}", file_path, e))?;

        // Additionally inject XMP XML packet for PersonInImage if people are tagged
        if let Some(people) = &meta.people {
            if !people.is_empty() {
                let _ = inject_xmp_into_jpeg(file_path, meta);
            }
        }

        Ok(())
    }

    fn embed_png_pure_rust(&self, file_path: &Path, meta: &TakeoutJsonMetadata) -> Result<(), String> {
        let mut metadata = little_exif::metadata::Metadata::new();

        let ts = meta
            .photo_taken_time
            .as_ref()
            .map(|t| &t.timestamp)
            .or_else(|| meta.creation_time.as_ref().map(|t| &t.timestamp));

        if let Some(ts_str) = ts {
            if let Some(formatted_time) = Self::format_timestamp(ts_str) {
                metadata.set_tag(little_exif::exif_tag::ExifTag::DateTimeOriginal(formatted_time));
            }
        }

        if let Some(desc) = Self::format_description(meta) {
            metadata.set_tag(little_exif::exif_tag::ExifTag::ImageDescription(desc));
        }

        metadata.write_to_file(file_path).map_err(|e| format!("Failed to write EXIF to PNG {:?}: {}", file_path, e))?;
        Ok(())
    }

    fn embed_webp_pure_rust(&self, file_path: &Path, meta: &TakeoutJsonMetadata) -> Result<(), String> {
        let mut metadata = little_exif::metadata::Metadata::new();

        let ts = meta
            .photo_taken_time
            .as_ref()
            .map(|t| &t.timestamp)
            .or_else(|| meta.creation_time.as_ref().map(|t| &t.timestamp));

        if let Some(ts_str) = ts {
            if let Some(formatted_time) = Self::format_timestamp(ts_str) {
                metadata.set_tag(little_exif::exif_tag::ExifTag::DateTimeOriginal(formatted_time));
            }
        }

        if let Some(desc) = Self::format_description(meta) {
            metadata.set_tag(little_exif::exif_tag::ExifTag::ImageDescription(desc));
        }

        metadata.write_to_file(file_path).map_err(|e| format!("Failed to write EXIF to WebP {:?}: {}", file_path, e))?;
        Ok(())
    }
}

fn inject_xmp_into_jpeg(file_path: &Path, meta: &TakeoutJsonMetadata) -> Result<(), String> {
    use std::fs;
    let bytes = fs::read(file_path).map_err(|e| format!("Failed to read file: {}", e))?;
    let mut jpeg = img_parts::jpeg::Jpeg::from_bytes(bytes.into()).map_err(|e| format!("Not a valid JPEG: {}", e))?;

    let xmp_xml = build_xmp_packet(meta);
    let mut xmp_segment_payload = Vec::new();
    xmp_segment_payload.extend_from_slice(b"http://ns.adobe.com/xap/1.0/\0");
    xmp_segment_payload.extend_from_slice(xmp_xml.as_bytes());

    // Replace or insert XMP segment
    let segments = jpeg.segments_mut();
    // APP1 marker is 0xE1
    let mut found_xmp = false;
    for seg in segments.iter_mut() {
        if seg.marker() == 0xE1 && seg.contents().starts_with(b"http://ns.adobe.com/xap/1.0/\0") {
            *seg = img_parts::jpeg::JpegSegment::new_with_contents(0xE1, xmp_segment_payload.clone().into());
            found_xmp = true;
            break;
        }
    }

    if !found_xmp {
        // Insert after SOI (at index 1 or 0)
        let insert_idx = if segments.is_empty() { 0 } else { 1 };
        segments.insert(insert_idx, img_parts::jpeg::JpegSegment::new_with_contents(0xE1, xmp_segment_payload.into()));
    }

    let mut out = Vec::new();
    jpeg.encoder().write_to(&mut out).map_err(|e| format!("Failed to encode JPEG: {}", e))?;
    fs::write(file_path, out).map_err(|e| format!("Failed to write modified JPEG: {}", e))?;
    Ok(())
}

fn build_xmp_packet(meta: &TakeoutJsonMetadata) -> String {
    let mut people_lis = String::new();
    if let Some(people) = &meta.people {
        for p in people {
            let name = p.name.trim();
            if !name.is_empty() {
                people_lis.push_str(&format!("        <rdf:li>{}</rdf:li>\n", quick_xml_escape(name)));
            }
        }
    }

    let desc = MetadataEngine::format_description(meta).unwrap_or_default();
    let escaped_desc = quick_xml_escape(&desc);

    format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="photos-migrator">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:dc="http://purl.org/dc/elements/1.1/"
    xmlns:iptcExt="http://iptc.org/std/Iptc4xmpExt/2008-02-29/">
   <dc:description>
    <rdf:Alt>
     <rdf:li xml:lang="x-default">{}</rdf:li>
    </rdf:Alt>
   </dc:description>
   <dc:subject>
    <rdf:Bag>
{}    </rdf:Bag>
   </dc:subject>
   <iptcExt:PersonInImage>
    <rdf:Bag>
{}    </rdf:Bag>
   </iptcExt:PersonInImage>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#,
        escaped_desc, people_lis, people_lis
    )
}

fn quick_xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
