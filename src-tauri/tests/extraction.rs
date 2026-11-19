use file_content_search::extract::{
    Document, ErrorKind, Limits, LocationKind, extract_bytes, extract_file,
};

fn fixture(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn read(name: &str) -> Document {
    extract_file(&fixture(name), Limits::default()).unwrap()
}

#[test]
fn chinese_text_and_locations() {
    let doc = read("chinese.txt");
    assert!(doc.text.contains("合同签署 ABC \"AND\" %_ 😀"));
    assert_eq!(doc.location_kind, LocationKind::Line);
    assert_eq!(doc.locations.len(), 2);
    assert!(doc.text[doc.locations[1].start..].starts_with("第二行"));
}

#[test]
fn strict_encoding_and_boms() {
    let text = "合同😀\r\n验收";
    for little in [true, false] {
        let mut bytes = if little {
            vec![0xff, 0xfe]
        } else {
            vec![0xfe, 0xff]
        };
        bytes.extend(text.encode_utf16().flat_map(|unit| {
            if little {
                unit.to_le_bytes()
            } else {
                unit.to_be_bytes()
            }
        }));
        assert_eq!(
            extract_bytes(&bytes, "TXT", Limits::default())
                .unwrap()
                .text,
            text
        );
    }
    let mut bytes = vec![0xef, 0xbb, 0xbf];
    bytes.extend(text.as_bytes());
    assert_eq!(
        extract_bytes(&bytes, "txt", Limits::default())
            .unwrap()
            .text,
        text
    );
    for bytes in [
        &[0xff, 0xff][..],
        &[0xff, 0xfe, 0],
        &[0xff, 0xfe, 0x00, 0xd8],
        b"binary\0data",
    ] {
        assert_eq!(
            extract_bytes(bytes, "txt", Limits::default())
                .unwrap_err()
                .kind,
            ErrorKind::Decode
        );
    }
}

#[test]
fn docx_relationship_namespaces_runs_and_table_boundaries() {
    for name in ["chinese.docx", "strict.docx"] {
        let doc = read(name);
        assert_eq!(
            doc.text,
            "合同签署 ABC & <测试> 😀\n表格甲\n\t表格乙\n\t\n末段\t制表\n换行\n"
        );
        assert_eq!(doc.location_kind, LocationKind::Paragraph);
        assert_eq!(doc.locations.len(), 4);
        assert!(doc.text[doc.locations[2].start..].starts_with("表格乙"));
    }
}

#[test]
fn rejects_bad_docx_without_partial_success() {
    for name in [
        "external.docx",
        "entity.docx",
        "truncated.docx",
        "wrong-namespace.docx",
    ] {
        assert_eq!(
            extract_file(&fixture(name), Limits::default())
                .unwrap_err()
                .kind,
            ErrorKind::Corrupt,
            "{name}"
        );
    }
    assert_eq!(
        extract_file(&fixture("deep.docx"), Limits::default())
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
    assert_eq!(
        extract_bytes(&[0xd0, 0xcf, 0x11, 0xe0], "docx", Limits::default())
            .unwrap_err()
            .kind,
        ErrorKind::Encrypted
    );
}

#[test]
fn limits_are_errors_not_truncation() {
    for limits in [
        Limits {
            source_bytes: 10,
            ..Limits::default()
        },
        Limits {
            text_bytes: 4,
            ..Limits::default()
        },
        Limits {
            xml_bytes: 30,
            ..Limits::default()
        },
        Limits {
            zip_entries: 1,
            ..Limits::default()
        },
        Limits {
            xml_depth: 2,
            ..Limits::default()
        },
        Limits {
            locations: 1,
            ..Limits::default()
        },
    ] {
        assert_eq!(
            extract_file(&fixture("chinese.docx"), limits)
                .unwrap_err()
                .kind,
            ErrorKind::Limit
        );
    }
    let limits = Limits {
        locations: 1,
        ..Limits::default()
    };
    assert_eq!(
        extract_bytes(b"a\nb", "txt", limits).unwrap_err().kind,
        ErrorKind::Limit
    );
}

#[test]
fn chinese_pdf_mapping_pages_and_columns() {
    let doc = read("chinese.pdf");
    assert!(doc.text.contains("合同签署 ABC 😀"), "{:?}", doc.text);
    assert!(doc.text[doc.locations[1].start..].contains("第二页：合同验收"));
    assert_eq!(doc.location_kind, LocationKind::Page);
    assert_eq!(doc.locations.len(), 2);
    let columns = read("columns.pdf");
    assert!(columns.text.find("左栏合同").unwrap() < columns.text.find("右栏验收").unwrap());
}

#[test]
fn pdf_failures_and_limits() {
    let mut pdf = pdf_extract::Document::load(fixture("chinese.pdf")).unwrap();
    let page = *pdf.get_pages().get(&1).unwrap();
    let dictionary = pdf.get_object_mut(page).unwrap().as_dict_mut().unwrap();
    let content = dictionary.get(b"Contents").unwrap().clone();
    dictionary.set(
        "Contents",
        pdf_extract::Object::Array(vec![content, pdf_extract::Object::Integer(42)]),
    );
    let mut bytes = Vec::new();
    pdf.save_to(&mut bytes).unwrap();
    assert_eq!(
        extract_bytes(&bytes, "pdf", Limits::default())
            .unwrap_err()
            .kind,
        ErrorKind::Corrupt
    );
    assert_eq!(
        extract_file(&fixture("empty.pdf"), Limits::default())
            .unwrap_err()
            .kind,
        ErrorKind::NoText
    );
    assert_eq!(
        extract_file(&fixture("encrypted.pdf"), Limits::default())
            .unwrap_err()
            .kind,
        ErrorKind::Encrypted
    );
    assert_eq!(
        extract_file(&fixture("broken-page.pdf"), Limits::default())
            .unwrap_err()
            .kind,
        ErrorKind::Corrupt
    );
    for limits in [
        Limits {
            text_bytes: 4,
            ..Limits::default()
        },
        Limits {
            pdf_pages: 1,
            ..Limits::default()
        },
    ] {
        assert_eq!(
            extract_file(&fixture("chinese.pdf"), limits)
                .unwrap_err()
                .kind,
            ErrorKind::Limit
        );
    }
}

#[test]
fn unsupported_empty_corrupt() {
    assert_eq!(
        extract_bytes(b"", "txt", Limits::default())
            .unwrap_err()
            .kind,
        ErrorKind::NoText
    );
    assert_eq!(
        extract_bytes(b"abc", "doc", Limits::default())
            .unwrap_err()
            .kind,
        ErrorKind::Unsupported
    );
    for extension in ["docx", "pdf"] {
        assert_eq!(
            extract_bytes(b"not a document", extension, Limits::default())
                .unwrap_err()
                .kind,
            ErrorKind::Corrupt
        );
    }
}

#[test]
fn thirty_mib_source_and_read_only() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("large.txt");
    let text = "合同签署 ABC 😀 ".repeat(30 * 1024 * 1024 / "合同签署 ABC 😀 ".len() + 1);
    std::fs::write(&path, text.as_bytes()).unwrap();
    let before = std::fs::metadata(&path).unwrap();
    let doc = extract_file(&path, Limits::default()).unwrap();
    assert_eq!(doc.text, text);
    let after = std::fs::metadata(&path).unwrap();
    assert_eq!(before.modified().unwrap(), after.modified().unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), text.as_bytes());
}

#[test]
fn thirty_mib_docx_and_pdf_sources() {
    use std::io::Write;
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let mut original =
        zip::ZipArchive::new(std::fs::File::open(fixture("chinese.docx")).unwrap()).unwrap();
    for index in 0..original.len() {
        zip.raw_copy_file(original.by_index(index).unwrap())
            .unwrap();
    }
    zip.start_file(
        "media/unused.bin",
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
    )
    .unwrap();
    zip.write_all(&vec![0u8; 30 * 1024 * 1024]).unwrap();
    let bytes = zip.finish().unwrap().into_inner();
    assert!(
        extract_bytes(&bytes, "docx", Limits::default())
            .unwrap()
            .text
            .contains("合同签署")
    );
    let mut pdf = pdf_extract::Document::load(fixture("chinese.pdf")).unwrap();
    pdf.add_object(pdf_extract::Stream::new(
        pdf_extract::Dictionary::new(),
        vec![0; 30 * 1024 * 1024],
    ));
    let mut bytes = Vec::new();
    pdf.save_to(&mut bytes).unwrap();
    assert!(bytes.len() >= 30 * 1024 * 1024);
    assert!(
        extract_bytes(&bytes, "pdf", Limits::default())
            .unwrap()
            .text
            .contains("合同签署")
    );
}
