use file_content_search::{
    extract::{Limits, extract_bytes},
    index::{Session, Store, statistics},
};

fn insert(store: &Store, name: &str, text: &str) {
    let doc = extract_bytes(text.as_bytes(), "txt", Limits::default()).unwrap();
    store.put(name, text.len() as u64, 1, &doc).unwrap();
}
fn all(session: &mut Session) -> Vec<String> {
    let mut paths = Vec::new();
    loop {
        let page = session.page(paths.len(), 7).unwrap();
        paths.extend(page.hits.into_iter().map(|h| h.path));
        if page.complete {
            break;
        }
    }
    paths
}

#[test]
fn inventory_counts_only_live_files_in_its_snapshot() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::open(temp.path()).unwrap();
    insert(&store, "a.txt", "one");
    insert(&store, "b.DOCX", "four");
    insert(&store, "c.pdf", "three");
    store.commit().unwrap();
    let original = store.reader.searcher();
    let stats = statistics(&original).unwrap();
    assert_eq!(
        (
            stats.total,
            stats.text,
            stats.docx,
            stats.pdf,
            stats.source_bytes
        ),
        (3, 1, 1, 1, 12)
    );
    insert(&store, "b.DOCX", "updated docx");
    store.remove("a.txt");
    store.commit().unwrap();
    let stats = statistics(&store.reader.searcher()).unwrap();
    assert_eq!(
        (
            stats.total,
            stats.text,
            stats.docx,
            stats.pdf,
            stats.source_bytes
        ),
        (2, 0, 1, 1, 17)
    );
    assert_eq!(statistics(&original).unwrap().total, 3);
}

#[test]
fn literal_unicode_candidates_pagination_snapshot_and_restart() {
    use file_content_search::index::key;
    assert_eq!(key(r"C:\Café\File.TXT"), key(r"c:\CAFÉ\file.txt"));
    assert_ne!(key(r"C:\Straße.txt"), key(r"C:\STRASSE.txt"));
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::open(temp.path()).unwrap();
    for i in 0..71 {
        insert(
            &store,
            &format!("{i}.txt"),
            "文件正文合同签署 AAAA AND \" % _ 😀𠮷",
        );
    }
    for i in 0..84 {
        insert(&store, &format!("false-{i}.txt"), "合同 同签 签署 AB BA");
    }
    store.commit().unwrap();
    for query in [
        "合",
        "合同",
        "合同签",
        "合同签署",
        "aaaa",
        "AND",
        "\"",
        "%",
        "_",
        "😀𠮷",
        " ",
    ] {
        let mut session = Session::new(&store.reader, 1, query).unwrap();
        let paths = all(&mut session);
        let expected = if ["合", "合同", " "].contains(&query) {
            155
        } else {
            71
        };
        assert_eq!(paths.len(), expected, "{query}");
        let unique: std::collections::HashSet<_> = paths.iter().collect();
        assert_eq!(unique.len(), paths.len());
        let preview = session.preview(0, 8).unwrap();
        assert!(
            preview.snippets[0]
                .parts
                .iter()
                .any(|p| p.matched && p.text.eq_ignore_ascii_case(query))
        );
    }
    assert!(all(&mut Session::new(&store.reader, 1, "ABA").unwrap()).is_empty());
    assert!(all(&mut Session::new(&store.reader, 1, "").unwrap()).is_empty());
    let mut old = Session::new(&store.reader, 10, "合同签署").unwrap();
    assert_eq!(old.page(0, 1).unwrap().hits.len(), 1);
    for i in 0..71 {
        store.remove(&format!("{i}.txt"));
    }
    insert(&store, "new.txt", "合同签署新版本");
    store.commit().unwrap();
    let mut count = 1;
    loop {
        let p = old.page(count, 50).unwrap();
        count += p.hits.len();
        if p.complete {
            break;
        }
    }
    assert_eq!(count, 71);
    assert!(
        !old.preview(0, 1).unwrap().snippets[0]
            .parts
            .iter()
            .any(|p| p.text.contains("新版本"))
    );
    assert_eq!(
        all(&mut Session::new(&store.reader, 11, "合同签署").unwrap()),
        ["new.txt"]
    );
    assert!(old.page(0, 50).is_err());
    assert!(old.preview(500, 8).is_err());
    assert_eq!(store.metadata().unwrap().len(), 85);
    drop(old);
    drop(store);
    let mut store = Store::open(temp.path()).unwrap();
    insert(&store, "uncommitted.txt", "rollback");
    store.rollback().unwrap();
    assert!(all(&mut Session::new(&store.reader, 1, "rollback").unwrap()).is_empty());
    assert_eq!(
        all(&mut Session::new(&store.reader, 1, "合同签署").unwrap()),
        ["new.txt"]
    );
}

#[test]
fn extraction_locations_and_cross_boundary_highlights() {
    let text = format!("{}{}{}", "合同", "甲".repeat(79), "合同");
    let excerpts = file_content_search::index::snippets(
        &text,
        "合同",
        file_content_search::extract::LocationKind::Line,
        &[],
        8,
    );
    assert_eq!(
        excerpts
            .iter()
            .flat_map(|s| &s.parts)
            .filter(|p| p.matched)
            .count(),
        2
    );
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::open(temp.path()).unwrap();
    for (ext, bytes) in [
        ("docx", include_bytes!("fixtures/chinese.docx").as_slice()),
        ("pdf", include_bytes!("fixtures/chinese.pdf").as_slice()),
    ] {
        let doc = extract_bytes(bytes, ext, Limits::default()).unwrap();
        store
            .put(&format!("sample.{ext}"), bytes.len() as u64, 0, &doc)
            .unwrap();
    }
    insert(&store, "lines.txt", "第一行😀\n合同签署\n第三行");
    store.commit().unwrap();
    let mut session = Session::new(&store.reader, 1, "😀\n合同").unwrap();
    assert_eq!(all(&mut session), ["lines.txt"]);
    let p = session.preview(0, 8).unwrap();
    assert_eq!(p.snippets[0].location, 1);
    assert!(
        p.snippets[0]
            .parts
            .iter()
            .any(|p| p.matched && p.text == "😀\n合同")
    );
    let mut session = Session::new(&store.reader, 2, "合同").unwrap();
    assert_eq!(all(&mut session).len(), 3);
}

#[test]
fn failed_commit_preserves_previous_snapshot_and_recovers_on_restart() {
    use std::os::windows::fs::OpenOptionsExt;
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::open(temp.path()).unwrap();
    insert(&store, "record.txt", "original content");
    store.commit().unwrap();
    insert(&store, "record.txt", "replacement content");
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(temp.path().join("meta.json"))
        .unwrap();
    assert!(store.commit().is_err());
    assert_eq!(
        all(&mut Session::new(&store.reader, 1, "original").unwrap()),
        ["record.txt"]
    );
    assert!(all(&mut Session::new(&store.reader, 2, "replacement").unwrap()).is_empty());
    drop(lock);
    drop(store);
    let mut store = Store::open(temp.path()).unwrap();
    assert_eq!(
        all(&mut Session::new(&store.reader, 1, "original").unwrap()),
        ["record.txt"]
    );
    insert(&store, "record.txt", "replacement content");
    store.commit().unwrap();
    assert_eq!(
        all(&mut Session::new(&store.reader, 2, "replacement").unwrap()),
        ["record.txt"]
    );
}
