use file_content_search::{
    engine::Engine,
    filesystem::{DataDirectory, Scope},
};
use std::{
    fs,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

fn test_scope(roots: Vec<PathBuf>, exclusions: Vec<PathBuf>) -> Scope {
    let mut scope = Scope::new(roots, exclusions);
    scope.extensions = file_content_search::extract::EXTENSIONS
        .iter()
        .map(|s| (*s).into())
        .collect();
    scope
}

fn await_index(engine: &Engine, expected: u64) {
    let started = Instant::now();
    loop {
        let status = engine.status();
        assert_ne!(status.phase, "error", "{}", status.message);
        if status.phase == "idle" && status.indexed == expected {
            return;
        }
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "{}: {} / {expected}, {}",
            status.phase,
            status.indexed,
            status.message
        );
        thread::sleep(Duration::from_millis(30));
    }
}
fn query_all(engine: &Engine, request: u64, query: &str) -> Vec<String> {
    let mut page = engine.search(request, query).unwrap();
    let mut names = Vec::new();
    loop {
        names.extend(page.hits.into_iter().map(|h| h.name));
        if page.complete {
            break;
        }
        page = engine.page(request, names.len()).unwrap();
    }
    names
}
#[test]
fn watching_replacement_deletion_rebuild_and_restart() {
    let temp = tempfile::tempdir().unwrap();
    let docs = temp.path().join("docs");
    fs::create_dir(&docs).unwrap();
    let data = temp.path().join("index");
    fs::write(docs.join("alpha.txt"), "first 合同").unwrap();
    fs::write(docs.join("broken.txt"), [0xff, 0xee]).unwrap();
    fs::create_dir(docs.join("node_modules")).unwrap();
    fs::write(docs.join("node_modules/hidden.txt"), "first 合同").unwrap();
    let scope = test_scope(vec![docs.clone()], vec![data.clone()]);
    let engine = Engine::start(scope.clone(), data.clone());
    engine.refresh(false).unwrap();
    await_index(&engine, 1);
    assert!(engine.status().failed >= 1);
    assert_eq!(query_all(&engine, 1, "合同"), ["alpha.txt"]);
    let bytes = fs::read(docs.join("alpha.txt")).unwrap();
    assert_eq!(bytes, "first 合同".as_bytes());
    fs::write(docs.join("beta.txt"), "second 合同").unwrap();
    await_index(&engine, 2);
    fs::write(docs.join("replacement.tmp"), "third 合同").unwrap();
    fs::remove_file(docs.join("alpha.txt")).unwrap();
    fs::rename(docs.join("replacement.tmp"), docs.join("alpha.txt")).unwrap();
    let start = Instant::now();
    let mut request = 2;
    loop {
        request += 1;
        if query_all(&engine, request, "third").len() == 1 {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(10));
        thread::sleep(Duration::from_millis(50));
    }
    fs::rename(docs.join("beta.txt"), docs.join("renamed.txt")).unwrap();
    fs::remove_file(docs.join("alpha.txt")).unwrap();
    await_index(&engine, 1);
    request += 1;
    assert_eq!(query_all(&engine, request, "合同"), ["renamed.txt"]);
    engine.refresh(true).unwrap();
    thread::sleep(Duration::from_millis(200));
    await_index(&engine, 1);
    request += 1;
    assert_eq!(query_all(&engine, request, "合同"), ["renamed.txt"]);
    engine.stop();
    drop(engine);
    fs::write(docs.join("renamed.txt"), "offline changed").unwrap();
    let engine = Engine::start(scope, data);
    await_index(&engine, 1);
    assert_eq!(query_all(&engine, 1, "offline changed"), ["renamed.txt"]);
    assert!(query_all(&engine, 2, "合同").is_empty());
    assert!(engine.search(1, "older").is_err());
    assert!(engine.previews(1, &[0], true).is_err());
    engine.stop();
}

#[test]
fn owned_directory_and_path_boundaries() {
    let temp = tempfile::tempdir().unwrap();
    let occupied = temp.path().join("occupied");
    fs::create_dir(&occupied).unwrap();
    fs::write(occupied.join("user.txt"), "keep").unwrap();
    assert!(DataDirectory::open(&occupied, true).is_err());
    assert_eq!(
        fs::read_to_string(occupied.join("user.txt")).unwrap(),
        "keep"
    );
    let root = temp.path().join("owned");
    let directory = DataDirectory::open(&root, true).unwrap();
    let generation = directory.new_generation().unwrap();
    directory.activate(&generation).unwrap();
    assert!(directory.remove_generation(&generation).is_err());
    assert!(directory.remove_generation(&occupied).is_err());
    let scope = test_scope(vec![temp.path().into()], vec![root.clone()]);
    assert!(!scope.includes(&root.join("text.txt")));
    assert!(!scope.includes(&temp.path().join("node_modules/a.txt")));
    assert!(
        scope
            .validate_file(&temp.path().join("../escape.txt"))
            .is_err()
    );
    fs::write(
        root.join("active.json"),
        r#"{"format":1,"generation":"../occupied"}"#,
    )
    .unwrap();
    assert!(directory.active().is_err());
    let missing = PathBuf::from(r"Z:\nonexistent-fcs-test-drive\index");
    if !missing.parent().unwrap().exists() {
        assert!(DataDirectory::open(&missing, true).is_err());
    }
}

#[test]
fn inaccessible_subtree_is_preserved_while_confirmed_deletions_are_removed() {
    use std::os::windows::fs::OpenOptionsExt;
    let temp = tempfile::tempdir().unwrap();
    let docs = temp.path().join("documents");
    fs::create_dir(&docs).unwrap();
    let blocked = docs.join("blocked");
    fs::create_dir(&blocked).unwrap();
    fs::write(blocked.join("keep.txt"), "preserved").unwrap();
    fs::write(docs.join("delete.txt"), "deleted").unwrap();
    let data = temp.path().join("index");
    let scope = test_scope(vec![docs.clone()], vec![data.clone()]);
    let engine = Engine::start(scope.clone(), data.clone());
    engine.refresh(false).unwrap();
    await_index(&engine, 2);
    engine.stop();
    fs::remove_file(docs.join("delete.txt")).unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .custom_flags(0x02000000)
        .open(&blocked)
        .unwrap();
    let engine = Engine::start(scope, data);
    await_index(&engine, 1);
    assert_eq!(query_all(&engine, 1, "preserved"), ["keep.txt"]);
    assert!(query_all(&engine, 2, "deleted").is_empty());
    assert!(!engine.status().complete);
    engine.stop();
    drop(lock);
}

#[test]
fn cancelled_rebuild_keeps_the_old_generation() {
    let temp = tempfile::tempdir().unwrap();
    let docs = temp.path().join("documents");
    fs::create_dir(&docs).unwrap();
    for i in 0..300 {
        fs::write(docs.join(format!("file-{i}.txt")), "旧版本合同".repeat(100)).unwrap();
    }
    let data = temp.path().join("index");
    let engine = Engine::start(test_scope(vec![docs], vec![data.clone()]), data.clone());
    engine.refresh(false).unwrap();
    await_index(&engine, 300);
    let active = fs::read(data.join("active.json")).unwrap();
    engine.refresh(true).unwrap();
    let start = Instant::now();
    while engine.status().phase != "rebuilding" {
        assert!(start.elapsed() < Duration::from_secs(3));
        thread::sleep(Duration::from_millis(1));
    }
    engine.cancel();
    while engine.status().phase != "paused" {
        assert!(start.elapsed() < Duration::from_secs(5));
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(fs::read(data.join("active.json")).unwrap(), active);
    assert_eq!(query_all(&engine, 1, "旧版本合同").len(), 300);
    engine.stop();
}

#[test]
fn content_event_forces_update_when_size_and_timestamp_are_unchanged() {
    let temp = tempfile::tempdir().unwrap();
    let docs = temp.path().join("documents");
    fs::create_dir(&docs).unwrap();
    let path = docs.join("record.txt");
    fs::write(&path, "before").unwrap();
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    let data = temp.path().join("index");
    let engine = Engine::start(test_scope(vec![docs], vec![data.clone()]), data);
    engine.refresh(false).unwrap();
    await_index(&engine, 1);
    fs::write(&path, "after!").unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(modified))
        .unwrap();
    let start = Instant::now();
    let mut request = 1;
    loop {
        if query_all(&engine, request, "after!").len() == 1 {
            break;
        }
        request += 1;
        assert!(start.elapsed() < Duration::from_secs(5));
        thread::sleep(Duration::from_millis(30));
    }
    assert!(query_all(&engine, request + 1, "before").is_empty());
    engine.stop();
}

#[test]
fn unreadable_file_excludes_previous_text_without_stopping_other_files() {
    use std::os::windows::fs::OpenOptionsExt;
    let temp = tempfile::tempdir().unwrap();
    let docs = temp.path().join("documents");
    fs::create_dir(&docs).unwrap();
    let path = docs.join("locked.txt");
    fs::write(&path, "old secret").unwrap();
    let data = temp.path().join("index");
    let scope = test_scope(vec![docs.clone()], vec![data.clone()]);
    let engine = Engine::start(scope.clone(), data.clone());
    engine.refresh(false).unwrap();
    await_index(&engine, 1);
    engine.stop();
    fs::write(&path, "modified but unreadable text").unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path)
        .unwrap();
    fs::write(docs.join("ok.txt"), "new text").unwrap();
    let engine = Engine::start(scope, data);
    await_index(&engine, 1);
    assert!(query_all(&engine, 1, "old secret").is_empty());
    assert_eq!(query_all(&engine, 2, "new text"), ["ok.txt"]);
    engine.stop();
    drop(lock);
}

#[test]
fn new_exclusions_remove_old_results_but_unavailable_roots_preserve_them() {
    let temp = tempfile::tempdir().unwrap();
    let first = temp.path().join("first");
    let second = temp.path().join("second");
    let hidden = first.join("hidden");
    fs::create_dir_all(&hidden).unwrap();
    fs::create_dir(&second).unwrap();
    fs::write(first.join("visible.txt"), "visible").unwrap();
    fs::write(hidden.join("excluded.txt"), "excluded").unwrap();
    fs::write(second.join("offline.txt"), "offline").unwrap();
    let data = temp.path().join("index");
    let engine = Engine::start(
        test_scope(vec![first.clone(), second], vec![]),
        data.clone(),
    );
    engine.refresh(false).unwrap();
    await_index(&engine, 3);
    engine.stop();
    let engine = Engine::start(test_scope(vec![first], vec![hidden]), data);
    await_index(&engine, 2);
    assert!(query_all(&engine, 1, "excluded").is_empty());
    assert_eq!(query_all(&engine, 2, "offline"), ["offline.txt"]);
    assert!(!engine.status().complete);
    engine.stop();
}

#[test]
fn json_is_rejected_and_old_json_records_are_removed_even_outside_current_roots() {
    use file_content_search::{
        extract::{ErrorKind, Limits, extract_bytes, extract_file},
        filesystem::stamp,
        index::Store,
    };
    let temp = tempfile::tempdir().unwrap();
    let docs = temp.path().join("documents");
    let outside = temp.path().join("outside-current-roots");
    fs::create_dir_all(docs.join("folder.json")).unwrap();
    fs::create_dir(&outside).unwrap();
    let data = temp.path().join("index");
    let directory = DataDirectory::open(&data, true).unwrap();
    let generation = directory.new_generation().unwrap();
    let mut store = Store::open(&generation).unwrap();
    let json = r#"{"value":"json marker"}"#;
    for (path, text) in [
        (docs.join("keep.txt"), "visible text"),
        (docs.join("folder.json/child.txt"), "nested text"),
        (outside.join("offline.txt"), "preserved text"),
        (outside.join("old.json"), json),
        (outside.join("upper.JSON"), json),
    ] {
        fs::write(&path, text).unwrap();
        let document = extract_bytes(text.as_bytes(), "txt", Limits::default()).unwrap();
        store
            .put(
                &path.to_string_lossy(),
                text.len() as u64,
                stamp(&fs::metadata(&path).unwrap()),
                &document,
            )
            .unwrap();
    }
    store.commit().unwrap();
    directory.activate(&generation).unwrap();
    drop(store);
    let new_json = docs.join("new.JsOn");
    fs::write(&new_json, json).unwrap();
    assert_eq!(
        extract_file(&new_json, Limits::default()).unwrap_err().kind,
        ErrorKind::Unsupported
    );
    let engine = Engine::start(test_scope(vec![docs], vec![data.clone()]), data);
    await_index(&engine, 3);
    assert!(query_all(&engine, 1, "json marker").is_empty());
    assert_eq!(query_all(&engine, 2, "visible text"), ["keep.txt"]);
    assert_eq!(query_all(&engine, 3, "nested text"), ["child.txt"]);
    assert_eq!(query_all(&engine, 4, "preserved text"), ["offline.txt"]);
    engine.stop();
    for path in [
        new_json,
        outside.join("old.json"),
        outside.join("upper.JSON"),
    ] {
        assert_eq!(fs::read_to_string(path).unwrap(), json);
    }
}

#[test]
fn production_scope_excludes_own_data_without_hiding_user_windows_notes() {
    let scope = Scope::production();
    assert!(!scope.includes(
        &std::env::temp_dir().join("FileContentSearch-demo-123-456/documents/generated.txt")
    ));
    let project = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    assert!(!scope.includes(&project.join(".tools/toolchain/source.rs")));
    let profile = PathBuf::from(std::env::var_os("USERPROFILE").unwrap());
    assert!(scope.includes(&profile.join("Documents/Windows/notes.txt")));
    let local = PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap());
    assert!(scope.includes(&local.join("unregistered-user-notes/note.txt")));
    assert!(!scope.includes(&local.join("FileContentSearch/WebView2/cache.txt")));
}

#[test]
fn selected_types_persist_reconcile_and_filter_before_pagination() {
    let temp = tempfile::tempdir().unwrap();
    let docs = temp.path().join("docs");
    fs::create_dir(&docs).unwrap();
    for n in 0..130 {
        fs::write(docs.join(format!("{n:03}.md")), "类型过滤 合同").unwrap();
    }
    for n in 0..75 {
        fs::write(docs.join(format!("{n:03}.TXT")), "类型过滤 合同").unwrap();
    }
    let data = temp.path().join("index");
    let scope = test_scope(vec![docs.clone()], vec![data.clone()]);
    let engine = Engine::start(scope.clone(), data.clone());
    engine.refresh(false).unwrap();
    await_index(&engine, 205);
    let mut page = engine
        .search_filtered(1, "合同", Some(vec!["txt".into()]))
        .unwrap();
    let mut count = 0;
    loop {
        assert!(page.hits.iter().all(|hit| hit.name.ends_with(".TXT")));
        count += page.hits.len();
        if page.complete {
            break;
        }
        page = engine.page(1, count).unwrap();
    }
    assert_eq!(count, 75);
    assert!(engine.set_extensions(vec!["json".into()]).is_err());
    assert!(engine.set_extensions(vec![]).is_err());
    assert!(
        engine
            .set_extensions(vec!["txt".into(), "txt".into()])
            .is_err()
    );
    engine.set_extensions(vec!["txt".into()]).unwrap();
    await_index(&engine, 75);
    assert!(engine.page(1, count).is_err());
    assert_eq!(query_all(&engine, 2, "合同").len(), 75);
    assert!(
        engine
            .search_filtered(3, "合同", Some(vec!["md".into()]))
            .unwrap()
            .hits
            .is_empty()
    );
    assert!(docs.join("000.md").exists());
    engine.stop();
    drop(engine);
    fs::write(docs.join("new.md"), "新增 合同").unwrap();
    let engine = Engine::start(scope, data);
    engine.set_extensions(vec!["txt".into()]).unwrap();
    await_index(&engine, 75);
    assert_eq!(engine.status().extensions, ["txt"]);
    engine
        .set_extensions(vec!["txt".into(), "md".into()])
        .unwrap();
    await_index(&engine, 206);
    assert_eq!(query_all(&engine, 1, "合同").len(), 206);
    engine.stop();
}

#[test]
fn recycle_selected_rejects_changes_and_removes_only_confirmed_result() {
    let temp = tempfile::tempdir().unwrap();
    let docs = temp.path().join("docs");
    fs::create_dir(&docs).unwrap();
    let file = docs.join("FileContentSearch-recycle-test.txt");
    fs::write(&file, "可恢复临时合成样本").unwrap();
    let data = temp.path().join("index");
    let scope = test_scope(vec![docs.clone()], vec![data.clone()]);
    let engine = Engine::start(scope.clone(), data);
    engine.refresh(false).unwrap();
    await_index(&engine, 1);
    let page = engine.search(1, "临时合成样本").unwrap();
    assert_eq!(page.hits.len(), 1);
    assert!(engine.recycle_selected(1, 999).is_err());
    assert!(engine.recycle_selected(0, 0).is_err());
    let metadata = fs::metadata(&file).unwrap();
    let old = file_content_search::index::FileMeta {
        path: file.to_string_lossy().into_owned(),
        size: metadata.len(),
        modified: file_content_search::filesystem::stamp(&metadata),
    };
    fs::write(&file, "文件已经修改，必须重新确认 临时合成样本").unwrap();
    assert!(file_content_search::recycle::recycle(&scope, &old).is_err());
    assert!(file.exists());
    let outside = temp.path().join("outside.txt");
    fs::write(&outside, "keep").unwrap();
    let outside_meta = fs::metadata(&outside).unwrap();
    let unsafe_file = file_content_search::index::FileMeta {
        path: outside.to_string_lossy().into_owned(),
        size: outside_meta.len(),
        modified: file_content_search::filesystem::stamp(&outside_meta),
    };
    assert!(file_content_search::recycle::recycle(&scope, &unsafe_file).is_err());
    let mut request = 1;
    let started = Instant::now();
    loop {
        request += 1;
        let page = engine.search(request, "必须重新确认").unwrap();
        if !page.hits.is_empty() {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(10));
        thread::sleep(Duration::from_millis(30));
    }
    engine.recycle_selected(request, 0).unwrap();
    assert!(!file.exists());
    assert_eq!(fs::read_to_string(&outside).unwrap(), "keep");
    await_index(&engine, 0);
    assert!(query_all(&engine, request + 1, "临时合成样本").is_empty());
    engine.stop();
}

#[test]
fn fresh_defaults_only_index_documents_and_saved_choices_survive_restart() {
    let temp = tempfile::tempdir().unwrap();
    let docs = temp.path().join("docs");
    fs::create_dir(&docs).unwrap();
    fs::write(docs.join("note.MD"), "默认类型 合同").unwrap();
    fs::write(docs.join("note.markdown"), "默认类型 合同").unwrap();
    fs::write(docs.join("note.txt"), "自选类型 合同").unwrap();
    fs::write(docs.join("code.rs"), "不应默认索引").unwrap();
    fs::write(docs.join("config.json"), "不应索引").unwrap();
    fs::write(
        docs.join("sample.docx"),
        include_bytes!("fixtures/chinese.docx"),
    )
    .unwrap();
    let data = temp.path().join("index");
    let scope = Scope::new(vec![docs], vec![data.clone()]);
    assert_eq!(scope.extensions, ["docx", "pdf", "md", "markdown"]);
    let engine = Engine::start(scope.clone(), data.clone());
    assert_eq!(engine.status().extensions, scope.extensions);
    assert_eq!(engine.status().default_extensions, scope.extensions);
    engine.refresh(false).unwrap();
    assert!(
        engine.refresh(true).is_err(),
        "queued tasks must reject duplicate submission"
    );
    await_index(&engine, 3);
    assert_eq!(query_all(&engine, 1, "默认类型").len(), 2);
    assert!(query_all(&engine, 2, "自选类型").is_empty());
    engine.set_extensions(vec!["txt".into()]).unwrap();
    await_index(&engine, 1);
    engine.stop();
    drop(engine);
    let engine = Engine::start(scope, data);
    await_index(&engine, 1);
    assert_eq!(engine.status().extensions, ["txt"]);
    assert_eq!(query_all(&engine, 1, "自选类型"), ["note.txt"]);
    engine.stop();
}
