//! Reading a release archive: a gzip tar stream, read to its end under
//! limits before anything from it is used.
use flate2::read::GzDecoder;
use std::io::{self, Read};
use tar::EntryType;

/// Limits on one archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_entries: usize,
    /// Decompressed bytes in total, tar headers and padding included.
    pub max_total: u64,
    /// The size of the wanted file.
    pub max_file: u64,
    /// Bytes in an entry path.
    pub max_path: usize,
}

/// The limits for release archives.
pub const RELEASE: Limits = Limits {
    max_entries: 16,
    max_total: 256 * 1024 * 1024,
    max_file: 200 * 1024 * 1024,
    max_path: 255,
};

/// The contents of the single top-level regular file `name` in the gzip
/// tar `data`. The whole stream is read first: more entries than allowed,
/// more decompressed bytes than allowed, a path that is too long, absolute
/// or holds `..`, a link or special file, no `name` or a second one, or a
/// truncated stream fail the archive. Other regular files (`README.md`,
/// `LICENSE`, macOS `._*` metadata) and directories are skipped.
pub fn extract(data: &[u8], name: &str, limits: &Limits) -> Result<Vec<u8>, String> {
    let fail = |why: &str| format!("bad archive: {why}");
    let counted = Counted {
        inner: GzDecoder::new(data),
        left: limits.max_total,
    };
    let mut archive = tar::Archive::new(counted);
    let mut found: Option<Vec<u8>> = None;
    let mut entries = 0usize;
    for entry in archive.entries().map_err(|e| fail(&reason(&e)))? {
        let mut entry = entry.map_err(|e| fail(&reason(&e)))?;
        entries += 1;
        if entries > limits.max_entries {
            return Err(fail(&format!("more than {} entries", limits.max_entries)));
        }
        let kind = entry.header().entry_type();
        if kind == EntryType::XGlobalHeader {
            continue;
        }
        let path = entry.path_bytes().into_owned();
        if path.len() > limits.max_path {
            return Err(fail(&format!(
                "an entry path is longer than {} bytes",
                limits.max_path
            )));
        }
        if path.starts_with(b"/") {
            return Err(fail("an entry has an absolute path"));
        }
        let path = path.strip_prefix(b"./").unwrap_or(&path);
        if path.split(|b| *b == b'/').any(|part| part == b"..") {
            return Err(fail("an entry path contains .."));
        }
        let regular = matches!(kind, EntryType::Regular | EntryType::Continuous);
        if !regular && kind != EntryType::Directory {
            return Err(fail("it contains a link or a special file"));
        }
        if !regular || path != name.as_bytes() {
            continue;
        }
        if found.is_some() {
            return Err(fail(&format!("it contains two {name} entries")));
        }
        let size = entry.size();
        if size > limits.max_file {
            return Err(fail(&format!(
                "{name} is larger than {} bytes",
                limits.max_file
            )));
        }
        let mut contents = Vec::with_capacity(size.min(16 * 1024 * 1024) as usize);
        entry
            .read_to_end(&mut contents)
            .map_err(|e| fail(&reason(&e)))?;
        if contents.len() as u64 != size {
            return Err(fail("the stream is truncated"));
        }
        found = Some(contents);
    }
    // The rest of the stream: tar padding and the gzip trailer, whose
    // checksum and length catch truncation and corruption.
    io::copy(&mut archive.into_inner(), &mut io::sink()).map_err(|e| fail(&reason(&e)))?;
    found.ok_or_else(|| fail(&format!("it has no top-level {name}")))
}

/// A reader that fails once more than `left` bytes come through.
struct Counted<R> {
    inner: R,
    left: u64,
}

const TOO_LARGE: &str = "it decompresses to more than the limit";

impl<R: Read> Read for Counted<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let size = self.inner.read(buf)?;
        self.left = self
            .left
            .checked_sub(size as u64)
            .ok_or_else(|| io::Error::other(TOO_LARGE))?;
        Ok(size)
    }
}

fn reason(error: &io::Error) -> String {
    if error.to_string() == TOO_LARGE {
        return TOO_LARGE.to_owned();
    }
    match error.kind() {
        io::ErrorKind::UnexpectedEof => "the stream is truncated".to_owned(),
        _ => "it is not a valid gzip tar stream".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{write::GzEncoder, Compression};

    /// A gzip tar with raw headers, so tests can write what `tar::Builder`
    /// refuses (`..`, absolute paths).
    fn tar_gz(entries: &[(&[u8], EntryType, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
        for (path, kind, data) in entries {
            let mut header = tar::Header::new_gnu();
            header.as_mut_bytes()[..path.len()].copy_from_slice(path);
            header.set_size(data.len() as u64);
            header.set_mode(0o755);
            header.set_entry_type(*kind);
            if *kind == EntryType::Symlink {
                header.set_link_name("/etc/passwd").unwrap();
            }
            header.set_cksum();
            builder.append(&header, *data).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    fn release(binary: &[u8]) -> Vec<u8> {
        tar_gz(&[
            (b"mailtriage", EntryType::Regular, binary),
            (b"LICENSE", EntryType::Regular, b"MIT"),
            (b"README.md", EntryType::Regular, b"readme"),
        ])
    }

    fn error(data: &[u8]) -> String {
        extract(data, "mailtriage", &RELEASE).unwrap_err()
    }

    #[test]
    fn the_binary_is_extracted_and_other_files_are_skipped() {
        assert_eq!(
            extract(&release(b"#!bin"), "mailtriage", &RELEASE).unwrap(),
            b"#!bin"
        );
        let with_metadata = tar_gz(&[
            (b"./", EntryType::Directory, b""),
            (b"./._mailtriage", EntryType::Regular, b"apple"),
            (b"./mailtriage", EntryType::Regular, b"#!bin"),
        ]);
        assert_eq!(
            extract(&with_metadata, "mailtriage", &RELEASE).unwrap(),
            b"#!bin"
        );
    }

    #[test]
    fn pax_headers_are_understood() {
        let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
        builder
            .append_pax_extensions([("LIBARCHIVE.xattr.com.apple.provenance", &b"x"[..])])
            .unwrap();
        let mut header = tar::Header::new_ustar();
        header.set_size(5);
        header.set_mode(0o755);
        builder
            .append_data(&mut header, "mailtriage", &b"#!bin"[..])
            .unwrap();
        let data = builder.into_inner().unwrap().finish().unwrap();
        assert_eq!(extract(&data, "mailtriage", &RELEASE).unwrap(), b"#!bin");
    }

    #[test]
    fn unsafe_entries_fail_the_archive() {
        assert_eq!(
            error(&tar_gz(&[(b"mailtriage", EntryType::Symlink, b"")])),
            "bad archive: it contains a link or a special file"
        );
        assert_eq!(
            error(&tar_gz(&[(b"mailtriage", EntryType::Link, b"")])),
            "bad archive: it contains a link or a special file"
        );
        assert_eq!(
            error(&tar_gz(&[(b"../mailtriage", EntryType::Regular, b"x")])),
            "bad archive: an entry path contains .."
        );
        assert_eq!(
            error(&tar_gz(&[(b"a/../../x", EntryType::Regular, b"x")])),
            "bad archive: an entry path contains .."
        );
        assert_eq!(
            error(&tar_gz(&[(
                b"/usr/bin/mailtriage",
                EntryType::Regular,
                b"x"
            )])),
            "bad archive: an entry has an absolute path"
        );
    }

    #[test]
    fn the_binary_must_be_there_exactly_once() {
        assert_eq!(
            error(&tar_gz(&[(b"bin/mailtriage", EntryType::Regular, b"x")])),
            "bad archive: it has no top-level mailtriage"
        );
        assert_eq!(
            error(&tar_gz(&[
                (b"mailtriage", EntryType::Regular, b"x"),
                (b"./mailtriage", EntryType::Regular, b"y"),
            ])),
            "bad archive: it contains two mailtriage entries"
        );
        assert_eq!(
            error(&tar_gz(&[(b"mailtriage", EntryType::Directory, b"")])),
            "bad archive: it has no top-level mailtriage"
        );
    }

    #[test]
    fn release_limits_are_the_specs() {
        assert_eq!(
            RELEASE,
            Limits {
                max_entries: 16,
                max_total: 256 * 1024 * 1024,
                max_file: 200 * 1024 * 1024,
                max_path: 255,
            }
        );
    }

    /// The same checks with small limits, so the test needs no 256 MB.
    #[test]
    fn limits_are_enforced() {
        let many: Vec<(&[u8], EntryType, &[u8])> =
            vec![(b"README.md", EntryType::Regular, b"r"); 17];
        assert_eq!(error(&tar_gz(&many)), "bad archive: more than 16 entries");
        let small = Limits {
            max_entries: 16,
            max_total: 64 * 1024,
            max_file: 32 * 1024,
            max_path: 255,
        };
        // A decompression bomb: little gzip, much tar.
        let zeros = vec![0u8; 100 * 1024];
        let bomb = tar_gz(&[(b"README.md", EntryType::Regular, &zeros)]);
        assert!(bomb.len() < 4096);
        assert_eq!(
            extract(&bomb, "mailtriage", &small).unwrap_err(),
            "bad archive: it decompresses to more than the limit"
        );
        let big = tar_gz(&[(b"mailtriage", EntryType::Regular, &zeros[..40 * 1024])]);
        assert_eq!(
            extract(&big, "mailtriage", &small).unwrap_err(),
            "bad archive: mailtriage is larger than 32768 bytes"
        );
        let long = vec![b'a'; 256];
        let mut header = tar::Header::new_gnu();
        header.set_size(0);
        let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
        builder
            .append_data(&mut header, String::from_utf8(long).unwrap(), &b""[..])
            .unwrap();
        let data = builder.into_inner().unwrap().finish().unwrap();
        assert_eq!(
            error(&data),
            "bad archive: an entry path is longer than 255 bytes"
        );
    }

    #[test]
    fn a_truncated_or_corrupt_stream_fails() {
        let whole = release(&vec![7u8; 50_000]);
        assert_eq!(
            error(&whole[..whole.len() - 10]),
            "bad archive: the stream is truncated"
        );
        assert_eq!(
            error(&whole[..whole.len() / 2]),
            "bad archive: the stream is truncated"
        );
        assert_eq!(
            error(b"not gzip at all"),
            "bad archive: it is not a valid gzip tar stream"
        );
        let mut corrupt = whole.clone();
        let at = corrupt.len() - 6;
        corrupt[at] ^= 0xff;
        assert!(extract(&corrupt, "mailtriage", &RELEASE).is_err());
    }
}
