//! Unpacking downloaded archives: `.tar.gz` (Linux and macOS binaries, sources, R builds for
//! Linux) and `.zip` (Windows binaries and R builds; V2, V5).

use std::io::{Read, Seek};
use std::path::Path;

/// Unpacks a `.tar.gz` or `.zip` held in memory into `dest`, telling them apart by their first
/// bytes. Entries that would land outside `dest` are refused.
pub fn unpack(bytes: &[u8], dest: &Path) -> Result<(), String> {
    if bytes.starts_with(b"PK\x03\x04") {
        unzip(std::io::Cursor::new(bytes), dest)
    } else {
        tar::Archive::new(flate2::read::GzDecoder::new(bytes))
            .unpack(dest)
            .map_err(|e| e.to_string())
    }
}

/// Unpacks a `.zip` into `dest`.
pub fn unzip(reader: impl Read + Seek, dest: &Path) -> Result<(), String> {
    zip::ZipArchive::new(reader)
        .and_then(|mut z| z.extract(dest))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn unpacks_zip_and_tar_gz() {
        let t = tempfile::tempdir().unwrap();
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("R6/DESCRIPTION", options).unwrap();
        zip.write_all(b"Package: R6\n").unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        unpack(&bytes, &t.path().join("z")).unwrap();
        assert_eq!(
            std::fs::read_to_string(t.path().join("z/R6/DESCRIPTION")).unwrap(),
            "Package: R6\n"
        );

        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        let mut header = tar::Header::new_gnu();
        header.set_size(12);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, "R6/DESCRIPTION", &b"Package: R6\n"[..])
            .unwrap();
        let bytes = builder.into_inner().unwrap().finish().unwrap();
        unpack(&bytes, &t.path().join("t")).unwrap();
        assert!(t.path().join("t/R6/DESCRIPTION").is_file());
    }

    #[test]
    fn refuses_entries_outside_the_destination() {
        let t = tempfile::tempdir().unwrap();
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        zip.start_file("../evil", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"x").unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        let _ = unpack(&bytes, &t.path().join("d"));
        assert!(!t.path().join("evil").exists());
    }
}
