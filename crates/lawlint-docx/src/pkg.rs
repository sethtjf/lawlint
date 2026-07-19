//! Read/write the OPC (zip) container while preserving every part.
//!
//! We read all parts into memory, mutate only the ones we must
//! (`word/document.xml`, `word/comments.xml`, `[Content_Types].xml`,
//! `word/_rels/document.xml.rels`), and rewrite the archive. Untouched parts
//! (theme, settings, styles, customXml, media, …) round-trip content-identical
//! — the whole point of not going through a lossy typed model.

use std::io::{Cursor, Read, Write};

use crate::DocxError;

/// One archive entry, kept in original order.
pub struct Part {
    pub name: String,
    pub data: Vec<u8>,
}

pub struct Package {
    pub parts: Vec<Part>,
}

impl Package {
    pub fn read(bytes: &[u8]) -> Result<Self, DocxError> {
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
        let mut parts = Vec::with_capacity(zip.len());
        for i in 0..zip.len() {
            let mut file = zip.by_index(i)?;
            if !file.is_file() {
                continue;
            }
            let name = file.name().to_string();
            let mut data = Vec::with_capacity(file.size() as usize);
            file.read_to_end(&mut data)?;
            parts.push(Part { name, data });
        }
        Ok(Package { parts })
    }

    pub fn get(&self, name: &str) -> Option<&[u8]> {
        self.parts
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.data.as_slice())
    }

    pub fn get_str(&self, name: &str) -> Result<Option<String>, DocxError> {
        match self.get(name) {
            Some(bytes) => Ok(Some(String::from_utf8(bytes.to_vec()).map_err(|_| {
                DocxError::Malformed(format!("{name} is not valid UTF-8"))
            })?)),
            None => Ok(None),
        }
    }

    /// Replace an existing part's bytes, or append it if new (new parts go last).
    pub fn set(&mut self, name: &str, data: Vec<u8>) {
        if let Some(part) = self.parts.iter_mut().find(|p| p.name == name) {
            part.data = data;
        } else {
            self.parts.push(Part {
                name: name.to_string(),
                data,
            });
        }
    }

    pub fn write(&self) -> Result<Vec<u8>, DocxError> {
        let mut out = Vec::new();
        {
            let mut zw = zip::ZipWriter::new(Cursor::new(&mut out));
            let options = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for part in &self.parts {
                zw.start_file(&part.name, options)?;
                zw.write_all(&part.data)?;
            }
            zw.finish()?;
        }
        Ok(out)
    }
}
