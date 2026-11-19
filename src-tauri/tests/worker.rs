use file_content_search::{extract::ErrorKind, worker::extract_pdf};
use std::{path::Path, sync::atomic::AtomicBool};

#[test]
fn actual_application_worker_has_bounded_protocol_and_pdf_results() {
    let exe = Path::new(env!("CARGO_BIN_EXE_file-content-search"));
    let cancelled = AtomicBool::new(false);
    let document = extract_pdf(
        exe,
        include_bytes!("fixtures/chinese.pdf").to_vec(),
        &cancelled,
    )
    .unwrap();
    assert!(document.text.contains("合同签署 ABC 😀"));
    assert_eq!(document.locations.len(), 2);
    let error = extract_pdf(
        exe,
        include_bytes!("fixtures/broken-page.pdf").to_vec(),
        &cancelled,
    )
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Corrupt);
    let cancelled = AtomicBool::new(true);
    assert_eq!(
        extract_pdf(
            exe,
            include_bytes!("fixtures/chinese.pdf").to_vec(),
            &cancelled
        )
        .unwrap_err()
        .kind,
        ErrorKind::Cancelled
    );
}
