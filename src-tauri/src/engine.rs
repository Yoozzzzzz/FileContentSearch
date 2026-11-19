use crate::{
    extract::{self, ErrorKind},
    filesystem::{self, DataDirectory, Scope},
    index::{self, FileMeta, Page, Preview, Result, Session, Store},
    worker,
};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use tantivy::IndexReader;

const COMMIT_FILES: usize = 256;

#[derive(Clone, Serialize)]
pub struct Failure {
    pub path: String,
    pub reason: String,
}
#[derive(Clone, Serialize)]
pub struct Status {
    pub extensions: Vec<String>,
    pub available_extensions: Vec<String>,
    pub default_extensions: Vec<String>,
    pub phase: String,
    pub ready: bool,
    pub complete: bool,
    pub indexed: u64,
    pub checked: u64,
    pub failed: u64,
    pub last_updated: u64,
    pub version: u64,
    pub roots: Vec<String>,
    pub exclusions: Vec<String>,
    pub failures: Vec<Failure>,
    pub message: String,
}
struct SearchState {
    extensions: Vec<String>,
    reader: Option<IndexReader>,
    session: Option<Session>,
}
struct Shared {
    status: Mutex<Status>,
    search: Mutex<SearchState>,
    latest: AtomicU64,
    cancel: AtomicBool,
    shutdown: AtomicBool,
    dirty: AtomicBool,
    maintenance_pending: AtomicBool,
}
pub struct Engine {
    shared: Arc<Shared>,
    commands: SyncSender<Command>,
    maintenance_lock: Mutex<()>,
    handle: Mutex<Option<JoinHandle<()>>>,
    pub scope: Scope,
}
enum Command {
    Refresh(bool),
    Types(Vec<String>, SyncSender<Result<()>>),
    Recycle(FileMeta, SyncSender<Result<()>>),
}
impl Engine {
    pub fn start(scope: Scope, data_root: PathBuf) -> Self {
        let shared = Arc::new(Shared {
            status: Mutex::new(Status {
                extensions: scope.extensions.clone(),
                available_extensions: extract::EXTENSIONS.iter().map(|s| (*s).into()).collect(),
                default_extensions: extract::DEFAULT_EXTENSIONS
                    .iter()
                    .map(|s| (*s).into())
                    .collect(),
                phase: "waiting".into(),
                ready: false,
                complete: false,
                indexed: 0,
                checked: 0,
                failed: 0,
                last_updated: 0,
                version: 0,
                roots: scope
                    .roots
                    .iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect(),
                exclusions: scope
                    .exclusions
                    .iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .chain([
                        "Windows / Program Files / 回收站 / .git / node_modules / 链接和云占位符"
                            .into(),
                    ])
                    .collect(),
                failures: Vec::new(),
                message: "首次建立索引后可搜索本机文档".into(),
            }),
            search: Mutex::new(SearchState {
                extensions: scope.extensions.clone(),
                reader: None,
                session: None,
            }),
            latest: AtomicU64::new(0),
            cancel: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
            dirty: AtomicBool::new(false),
            maintenance_pending: AtomicBool::new(false),
        });
        let (commands, receiver) = mpsc::sync_channel(1);
        let thread_shared = shared.clone();
        let thread_scope = scope.clone();
        let handle =
            thread::spawn(move || coordinator(thread_shared, thread_scope, data_root, receiver));
        Self {
            shared,
            commands,
            maintenance_lock: Mutex::new(()),
            handle: Mutex::new(Some(handle)),
            scope,
        }
    }
    pub fn status(&self) -> Status {
        expire_session(&self.shared);
        self.shared.status.lock().unwrap().clone()
    }
    pub fn statistics(&self) -> Result<index::Statistics> {
        let searcher = self
            .shared
            .search
            .lock()
            .unwrap()
            .reader
            .as_ref()
            .ok_or("尚未建立索引")?
            .searcher();
        index::statistics(&searcher)
    }
    pub fn refresh(&self, rebuild: bool) -> Result<()> {
        let mut status = self.shared.status.lock().unwrap();
        if matches!(
            status.phase.as_str(),
            "queued" | "scanning" | "rebuilding" | "starting"
        ) {
            return Err("索引任务正在运行".into());
        }
        self.commands
            .try_send(Command::Refresh(rebuild))
            .map_err(|_| "索引任务已在队列中")?;
        status.phase = "queued".into();
        status.message = if rebuild {
            "重建任务已排队；仍可搜索已提交的索引"
        } else {
            "索引刷新已排队"
        }
        .into();
        Ok(())
    }
    pub fn cancel(&self) {
        self.shared.cancel.store(true, Ordering::Release);
    }
    pub fn set_extensions(&self, extensions: Vec<String>) -> Result<()> {
        extract::validate_extensions(&extensions)?;
        self.maintenance(|reply| Command::Types(extensions, reply))
    }
    fn maintenance(&self, command: impl FnOnce(SyncSender<Result<()>>) -> Command) -> Result<()> {
        let _operation = self
            .maintenance_lock
            .try_lock()
            .map_err(|_| "已有文件管理操作正在处理")?;
        let (reply, receiver) = mpsc::sync_channel(1);
        // Stop at the current file boundary before changing the writer's policy.
        self.shared
            .maintenance_pending
            .store(true, Ordering::Release);
        self.cancel();
        if self.commands.try_send(command(reply)).is_err() {
            self.shared
                .maintenance_pending
                .store(false, Ordering::Release);
            return Err("已有操作等待处理，请稍后重试".into());
        }
        receiver.recv().map_err(|_| "索引引擎已停止")?
    }
    pub fn recycle_selected(&self, request: u64, id: usize) -> Result<()> {
        let meta = self.with_session(request, |s| s.file_meta(id))?;
        self.scope.validate_file(Path::new(&meta.path))?;
        self.maintenance(|reply| Command::Recycle(meta, reply))
    }
    pub fn request_stop(&self) {
        self.shared.shutdown.store(true, Ordering::Release);
        self.cancel();
    }
    pub fn stop(&self) {
        self.request_stop();
        if let Some(handle) = self.handle.lock().unwrap().take() {
            let _ = handle.join();
        }
    }
    pub fn search(&self, request: u64, query: &str) -> Result<Page> {
        self.search_filtered(request, query, None)
    }
    pub fn search_filtered(
        &self,
        request: u64,
        query: &str,
        extensions: Option<Vec<String>>,
    ) -> Result<Page> {
        if let Some(ref types) = extensions {
            extract::validate_extensions(types)?;
        }
        if request == 0 || request > 9_007_199_254_740_991 {
            return Err("无效查询编号".into());
        }
        let old = self.shared.latest.fetch_max(request, Ordering::AcqRel);
        if request <= old {
            return Err("查询已过期".into());
        }
        let mut state = self.shared.search.lock().unwrap();
        if self.shared.latest.load(Ordering::Acquire) != request {
            return Err("查询已过期".into());
        }
        state.session = None;
        let reader = state.reader.as_ref().ok_or("尚未建立索引")?;
        let types = state
            .extensions
            .iter()
            .filter(|ext| extensions.as_ref().is_none_or(|types| types.contains(ext)))
            .cloned()
            .collect();
        let mut session = Session::filtered(reader, request, query, Some(types))?;
        let page = session.page(0, 50)?;
        if self.shared.latest.load(Ordering::Acquire) != request {
            return Err("查询已过期".into());
        }
        state.session = Some(session);
        Ok(page)
    }
    fn with_session<T>(
        &self,
        request: u64,
        f: impl FnOnce(&mut Session) -> Result<T>,
    ) -> Result<T> {
        let mut state = self.shared.search.lock().unwrap();
        let session = state.session.as_mut().ok_or("查询快照已释放，请重新搜索")?;
        if session.request != request || self.shared.latest.load(Ordering::Acquire) != request {
            return Err("查询已过期".into());
        }
        if session.touched.elapsed() > Duration::from_secs(180) {
            state.session = None;
            return Err("查询已闲置三分钟，请重新搜索".into());
        }
        f(session)
    }
    pub fn page(&self, request: u64, offset: usize) -> Result<Page> {
        self.with_session(request, |s| s.page(offset, 50))
    }
    pub fn previews(&self, request: u64, ids: &[usize], detail: bool) -> Result<Vec<Preview>> {
        if ids.is_empty() || ids.len() > if detail { 1 } else { 24 } {
            return Err("预览请求数量超限".into());
        }
        self.with_session(request, |s| {
            ids.iter()
                .map(|&id| s.preview(id, if detail { 8 } else { 1 }))
                .collect()
        })
    }
    pub fn selected_path(&self, request: u64, id: usize) -> Result<PathBuf> {
        let path = PathBuf::from(self.with_session(request, |s| s.path(id))?);
        self.scope.validate_file(&path)?;
        Ok(path)
    }
}
impl Drop for Engine {
    fn drop(&mut self) {
        self.stop();
    }
}

fn expire_session(shared: &Shared) {
    if let Ok(mut search) = shared.search.try_lock()
        && search
            .session
            .as_ref()
            .is_some_and(|session| session.touched.elapsed() > Duration::from_secs(180))
    {
        search.session = None;
    }
}

fn report(shared: &Shared, path: &Path, reason: impl Into<String>) {
    let mut status = shared.status.lock().unwrap();
    status.failed += 1;
    if status.failures.len() == 50 {
        status.failures.remove(0);
    }
    status.failures.push(Failure {
        path: path.to_string_lossy().into_owned(),
        reason: reason.into(),
    });
}
fn publish(shared: &Shared, store: &Store, complete: bool) {
    let mut state = shared.search.lock().unwrap();
    state.reader = Some(store.reader.clone());
    let mut status = shared.status.lock().unwrap();
    status.ready = true;
    status.indexed = store.reader.searcher().num_docs();
    status.version += 1;
    status.complete = complete;
    status.last_updated = store.last_updated;
}
fn watch(
    shared: Arc<Shared>,
    scope: Scope,
    events: SyncSender<Vec<PathBuf>>,
) -> Result<RecommendedWatcher> {
    let watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
        queue_event(&shared, &scope, &events, result);
    })
    .map_err(|_| "文件监听初始化失败")?;
    Ok(watcher)
}
fn queue_event(
    shared: &Shared,
    scope: &Scope,
    events: &SyncSender<Vec<PathBuf>>,
    result: notify::Result<notify::Event>,
) {
    if shared.shutdown.load(Ordering::Relaxed) {
        return;
    }
    match result {
        Ok(event) => {
            if matches!(event.kind, notify::EventKind::Access(_)) {
                return;
            }
            if event.need_rescan()
                || (event.paths.is_empty()
                    && matches!(
                        event.kind,
                        notify::EventKind::Any | notify::EventKind::Other
                    ))
            {
                shared.dirty.store(true, Ordering::Release);
            }
            let paths: Vec<_> = event
                .paths
                .into_iter()
                .filter(|p| scope.includes(p))
                .take(257)
                .collect();
            if paths.len() > 256 || (!paths.is_empty() && events.try_send(paths).is_err()) {
                shared.dirty.store(true, Ordering::Release);
            }
        }
        Err(_) => {
            shared.dirty.store(true, Ordering::Release);
        }
    }
}

fn coordinator(shared: Arc<Shared>, mut scope: Scope, root: PathBuf, commands: Receiver<Command>) {
    let (event_sender, events) = mpsc::sync_channel(256);
    let mut watcher = None;
    let mut store: Option<Store> = None;
    let mut directory = None;
    let mut active = None;
    let mut known = HashMap::new();
    let mut paused = false;
    let mut last_reconcile = Instant::now();
    let mut first = root.join(".owner").exists();
    loop {
        if shared.shutdown.load(Ordering::Acquire) {
            break;
        }
        expire_session(&shared);
        let command = if first {
            first = false;
            Some(Command::Refresh(false))
        } else {
            commands.try_recv().ok()
        };
        let mut paths = Vec::new();
        let mut force = false;
        let mut rebuild = false;
        let mut type_reply = None;
        if let Some(command) = command {
            if !matches!(command, Command::Refresh(_)) {
                shared.maintenance_pending.store(false, Ordering::Release);
            }
            let requested_types = match command {
                Command::Refresh(value) => {
                    rebuild = value;
                    None
                }
                Command::Types(types, reply) => {
                    type_reply = Some(reply);
                    Some(types)
                }
                Command::Recycle(meta, reply) => {
                    let result = (|| -> Result<()> {
                        let store = store.as_mut().ok_or("索引尚未就绪")?;
                        crate::recycle::recycle(&scope, &meta)?;
                        shared.search.lock().unwrap().session = None;
                        store.remove(&meta.path);
                        if let Err(error) = store.commit() {
                            shared.search.lock().unwrap().reader = None;
                            shared.status.lock().unwrap().ready = false;
                            return Err(format!("文件已移入回收站，但索引刷新失败：{error}"));
                        }
                        known.remove(&index::key(&meta.path));
                        publish(&shared, store, false);
                        Ok(())
                    })();
                    if result.is_err()
                        && let Some(store) = &mut store
                    {
                        let _ = store.rollback();
                    }
                    let _ = reply.send(result);
                    shared.cancel.store(false, Ordering::Release);
                    if shared.maintenance_pending.load(Ordering::Acquire) {
                        shared.cancel.store(true, Ordering::Release);
                    }
                    last_reconcile = Instant::now() - Duration::from_secs(300);
                    paused = false;
                    continue;
                }
            };
            shared.cancel.store(false, Ordering::Release);
            if shared.maintenance_pending.load(Ordering::Acquire) {
                shared.cancel.store(true, Ordering::Release);
            }
            paused = false;
            let mut status = shared.status.lock().unwrap();
            status.phase = "starting".into();
            status.message = if requested_types.is_some() {
                "正在保存类型选择并清理旧索引"
            } else if rebuild {
                "正在准备重建；仍可搜索已提交的索引"
            } else {
                "正在打开索引并准备复核"
            }
            .into();
            status.checked = 0;
            status.failed = 0;
            status.failures.clear();
            drop(status);
            if directory.is_none() {
                let loaded = (|| -> Result<()> {
                    let data = DataDirectory::open(&root, true)?;
                    scope.extensions = data.extensions(&scope.extensions)?;
                    shared.search.lock().unwrap().extensions = scope.extensions.clone();
                    let existing = data.active()?;
                    let path = match existing {
                        Some(path) => path,
                        None => {
                            let p = data.new_generation()?;
                            let mut initial = Store::open(&p)?;
                            initial.commit()?;
                            data.activate(&p)?;
                            p
                        }
                    };
                    let mut opened = Store::open(&path)?;
                    for error in data.cleanup_inactive() {
                        report(&shared, &root, error);
                    }
                    known = opened.metadata()?;
                    let excluded: Vec<_> = known
                        .iter()
                        .filter(|(_, meta)| {
                            let path = Path::new(&meta.path);
                            scope.excluded(path)
                                || !extract::matches_extensions(path, &scope.extensions)
                        })
                        .map(|(id, meta)| (id.clone(), meta.path.clone()))
                        .collect();
                    if !excluded.is_empty() {
                        let count = excluded.len();
                        for (index, (id, path)) in excluded.into_iter().enumerate() {
                            opened.remove(&path);
                            known.remove(&id);
                            if (index + 1).is_multiple_of(COMMIT_FILES) {
                                opened.commit()?;
                            }
                        }
                        if !count.is_multiple_of(COMMIT_FILES) {
                            opened.commit()?;
                        }
                    }
                    publish(&shared, &opened, false);
                    store = Some(opened);
                    active = Some(path);
                    directory = Some(data);
                    shared.status.lock().unwrap().extensions = scope.extensions.clone();
                    Ok(())
                })();
                if let Err(error) = loaded {
                    if let Some(reply) = type_reply.take() {
                        let _ = reply.send(Err(error.clone()));
                    }
                    set_error(&shared, error);
                    paused = true;
                    continue;
                }
            }
            if let Some(types) = requested_types {
                let changed = (|| -> Result<()> {
                    let data = directory.as_ref().unwrap();
                    data.save_extensions(&types)?;
                    scope.extensions = types;
                    {
                        let mut state = shared.search.lock().unwrap();
                        state.session = None;
                        state.extensions = scope.extensions.clone();
                    }
                    shared.status.lock().unwrap().extensions = scope.extensions.clone();
                    let store = store.as_mut().unwrap();
                    let excluded: Vec<_> = known
                        .values()
                        .filter(|meta| {
                            !extract::matches_extensions(Path::new(&meta.path), &scope.extensions)
                        })
                        .map(|meta| meta.path.clone())
                        .collect();
                    let total = excluded.len();
                    for (number, batch) in excluded.chunks(COMMIT_FILES).enumerate() {
                        for path in batch {
                            store.remove(path);
                        }
                        store.commit()?;
                        for path in batch {
                            known.remove(&index::key(path));
                        }
                        shared.status.lock().unwrap().message = format!(
                            "正在应用类型：已清理 {} / {total} 条旧索引",
                            ((number + 1) * COMMIT_FILES).min(total)
                        );
                    }
                    publish(&shared, store, false);
                    Ok(())
                })();
                if let Some(reply) = type_reply.take() {
                    let _ = reply.send(changed.clone());
                }
                if let Err(error) = changed {
                    if let Some(store) = &mut store {
                        let _ = store.rollback();
                    }
                    set_error(&shared, error);
                    paused = true;
                    continue;
                }
            }
            if watcher.is_none() {
                match watch(shared.clone(), scope.clone(), event_sender.clone()) {
                    Ok(mut handle) => {
                        for path in &scope.roots {
                            if handle.watch(path, RecursiveMode::Recursive).is_err() {
                                report(&shared, path, "无法监听此目录，将使用周期复核");
                            }
                        }
                        watcher = Some(handle);
                    }
                    Err(e) => report(&shared, &root, e),
                }
            }
            paths = scope.roots.clone();
        } else if !paused && store.is_some() {
            let dirty = shared.dirty.swap(false, Ordering::AcqRel);
            if dirty || last_reconcile.elapsed() >= Duration::from_secs(300) {
                paths = scope.roots.clone();
                force = dirty;
                while events.try_recv().is_ok() {}
            } else {
                let mut unique = HashSet::new();
                while let Ok(batch) = events.try_recv() {
                    for p in batch {
                        unique.insert(p);
                    }
                    if unique.len() > 1024 {
                        shared.dirty.store(true, Ordering::Release);
                        break;
                    }
                }
                paths = unique.into_iter().collect();
                force = true;
                paths.sort_by_key(|p| p.components().count());
                let mut compact: Vec<PathBuf> = Vec::new();
                for path in paths {
                    if !compact.iter().any(|p| path.starts_with(p)) {
                        compact.push(path);
                    }
                }
                paths = compact;
            }
        }
        if paths.is_empty() {
            thread::sleep(Duration::from_millis(150));
            continue;
        }
        if shared.cancel.load(Ordering::Acquire) {
            paused = true;
            shared.status.lock().unwrap().phase = "paused".into();
            continue;
        }
        let data = directory.as_ref().expect("initialized index directory");
        let result = (|| -> Result<bool> {
            data.check_space()?;
            {
                let mut status = shared.status.lock().unwrap();
                status.phase = if rebuild { "rebuilding" } else { "scanning" }.into();
                status.message = if rebuild {
                    "重建期间继续使用旧索引"
                } else {
                    "正在复核和更新文档"
                }
                .into();
            }
            if rebuild {
                let new_path = data.new_generation()?;
                let mut replacement = Store::open(&new_path)?;
                let mut new_known = HashMap::new();
                let outcome = Scan::new(
                    &shared,
                    &scope,
                    data,
                    &mut replacement,
                    &mut new_known,
                    false,
                )
                .run(&paths, true);
                if matches!(outcome, Ok(true))
                    && !shared.cancel.load(Ordering::Acquire)
                    && !shared.shutdown.load(Ordering::Acquire)
                {
                    replacement.commit()?;
                    data.activate(&new_path)?;
                    let old_path = active.replace(new_path).unwrap();
                    {
                        shared.search.lock().unwrap().session = None;
                    }
                    store = Some(replacement);
                    known = new_known;
                    publish(&shared, store.as_ref().unwrap(), true);
                    if let Err(e) = data.remove_generation(&old_path) {
                        report(&shared, &old_path, e);
                    }
                    Ok(true)
                } else {
                    drop(replacement);
                    if let Err(e) = data.remove_generation(&new_path) {
                        report(&shared, &new_path, e);
                    }
                    outcome.map(|_| false)
                }
            } else {
                Scan::new(
                    &shared,
                    &scope,
                    data,
                    store.as_mut().unwrap(),
                    &mut known,
                    true,
                )
                .run(&paths, force)
            }
        })();
        if paths == scope.roots {
            last_reconcile = Instant::now();
        }
        match result {
            Ok(complete) => {
                let complete = complete
                    && (paths != scope.roots
                        || known
                            .values()
                            .all(|file| scope.root_available(Path::new(&file.path))));
                let mut status = shared.status.lock().unwrap();
                paused = shared.cancel.load(Ordering::Acquire);
                status.phase = if paused { "paused" } else { "idle" }.into();
                if paths == scope.roots {
                    status.complete = complete;
                }
                status.message = if paused {
                    "任务已取消；已提交结果保留，点击刷新继续"
                } else if complete {
                    "索引已更新；运行期间自动监听变化"
                } else if rebuild {
                    "重建未完整完成，未切换索引；旧索引部分内容可能过期，详见失败记录"
                } else {
                    "部分目录未完成复核，保留未确认删除的旧记录"
                }
                .into();
            }
            Err(error) => {
                if let Some(store) = &mut store {
                    let _ = store.rollback();
                    if let Ok(meta) = store.metadata() {
                        known = meta;
                    }
                }
                set_error(&shared, error);
                paused = true;
            }
        }
    }
    drop(watcher);
}
fn set_error(shared: &Shared, message: String) {
    let mut status = shared.status.lock().unwrap();
    status.phase = "error".into();
    status.message = message;
    status.complete = false;
}

struct Scan<'a> {
    shared: &'a Shared,
    scope: &'a Scope,
    data: &'a DataDirectory,
    store: &'a mut Store,
    known: &'a mut HashMap<String, FileMeta>,
    pending: usize,
    bytes: usize,
    visible: bool,
    last_flush: Instant,
}

impl<'a> Scan<'a> {
    fn new(
        shared: &'a Shared,
        scope: &'a Scope,
        data: &'a DataDirectory,
        store: &'a mut Store,
        known: &'a mut HashMap<String, FileMeta>,
        visible: bool,
    ) -> Self {
        Self {
            shared,
            scope,
            data,
            store,
            known,
            pending: 0,
            bytes: 0,
            visible,
            last_flush: Instant::now(),
        }
    }
    fn run(&mut self, roots: &[PathBuf], force: bool) -> Result<bool> {
        let mut complete = true;
        for root in roots {
            let mut seen = HashSet::new();
            let finished = self.walk(root, force, &mut seen, 0)?;
            complete &= finished;
            if !self.shared.cancel.load(Ordering::Acquire)
                && !self.shared.shutdown.load(Ordering::Acquire)
            {
                let root_key = PathBuf::from(index::key(&root.to_string_lossy()));
                let removed: Vec<_> = self
                    .known
                    .iter()
                    .filter(|(id, _)| {
                        Path::new(id.as_str()).starts_with(&root_key) && !seen.contains(*id)
                    })
                    .map(|(id, m)| (id.clone(), m.path.clone()))
                    .collect();
                for (id, path) in removed {
                    self.store.remove(&path);
                    self.known.remove(&id);
                    self.pending += 1;
                    if self.pending >= COMMIT_FILES {
                        self.flush()?;
                    }
                }
            }
            if self.shared.cancel.load(Ordering::Acquire) {
                complete = false;
                break;
            }
        }
        self.flush()?;
        Ok(complete)
    }
}
impl Scan<'_> {
    fn protect(&self, path: &Path, seen: &mut HashSet<String>) {
        let path = PathBuf::from(index::key(&path.to_string_lossy()));
        seen.extend(
            self.known
                .keys()
                .filter(|id| Path::new(id.as_str()).starts_with(&path))
                .cloned(),
        );
    }
    fn flush(&mut self) -> Result<()> {
        if self.pending == 0 {
            return Ok(());
        }
        self.data.check_space()?;
        self.store.commit()?;
        self.pending = 0;
        self.bytes = 0;
        self.last_flush = Instant::now();
        if self.visible {
            let complete = self.shared.status.lock().unwrap().complete;
            publish(self.shared, self.store, complete);
        }
        Ok(())
    }
    fn walk(
        &mut self,
        path: &Path,
        force: bool,
        seen: &mut HashSet<String>,
        depth: usize,
    ) -> Result<bool> {
        if self.shared.cancel.load(Ordering::Acquire)
            || self.shared.shutdown.load(Ordering::Acquire)
        {
            return Ok(false);
        }
        if !self.scope.includes(path) {
            return Ok(true);
        }
        if depth > 128 {
            self.protect(path, seen);
            report(self.shared, path, "目录层级超过 128，未继续进入");
            return Ok(false);
        }
        let metadata = match fs::symlink_metadata(path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Missing roots and inaccessible parents cannot certify deletion.
                let confirmed = path
                    .parent()
                    .and_then(|p| fs::read_dir(p).ok())
                    .is_some_and(|entries| {
                        let mut complete = true;
                        for entry in entries {
                            match entry {
                                Ok(e) if e.path() == path => return false,
                                Ok(_) => {}
                                Err(_) => complete = false,
                            }
                        }
                        complete
                    });
                if !confirmed {
                    self.protect(path, seen);
                    report(self.shared, path, "目录离线或无法确认文件是否删除");
                }
                return Ok(confirmed);
            }
            Err(_) => {
                let id = index::key(&path.to_string_lossy());
                if let Some(old) = self.known.remove(&id) {
                    self.store.remove(&old.path);
                    self.pending += 1;
                    seen.insert(id);
                    report(self.shared, path, "文件无法读取，已从有效搜索中排除旧内容");
                    return Ok(true);
                }
                self.protect(path, seen);
                report(self.shared, path, "无法读取文件或目录，未清理其旧记录");
                return Ok(false);
            }
        };
        if filesystem::unsafe_attributes(&metadata) {
            report(self.shared, path, "跳过链接、联接点或云端占位符");
            return Ok(true);
        }
        if metadata.is_dir() {
            let entries = match fs::read_dir(path) {
                Ok(entries) => entries,
                Err(_) => {
                    self.protect(path, seen);
                    report(self.shared, path, "目录访问失败，保留旧记录");
                    return Ok(false);
                }
            };
            let mut complete = true;
            for entry in entries {
                match entry {
                    Ok(entry) => {
                        if !self.walk(&entry.path(), force, seen, depth + 1)? {
                            complete = false;
                        }
                    }
                    Err(_) => {
                        self.protect(path, seen);
                        report(self.shared, path, "目录枚举未完成，保留旧记录");
                        complete = false;
                    }
                }
                if self.shared.cancel.load(Ordering::Acquire) {
                    return Ok(false);
                }
            }
            return Ok(complete);
        }
        let extension = path
            .extension()
            .and_then(|p| p.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if extension == "doc" {
            report(self.shared, path, "首版不支持旧版 .doc，请转换为 DOCX");
            return Ok(true);
        }
        if !extract::matches_extensions(path, &self.scope.extensions) || !metadata.is_file() {
            return Ok(true);
        }
        let path_string = path.to_string_lossy().into_owned();
        let id = index::key(&path_string);
        seen.insert(id.clone());
        self.shared.status.lock().unwrap().checked += 1;
        if !force
            && self.known.get(&id).is_some_and(|m| {
                m.size == metadata.len() && m.modified == filesystem::stamp(&metadata)
            })
        {
            return Ok(true);
        }
        let mut result = Err(extract::ExtractError::new(
            ErrorKind::Changed,
            "读取期间文件变化",
        ));
        let mut verified = None;
        for attempt in 0..3 {
            let before = fs::symlink_metadata(path).ok();
            result = match self.scope.validate_file(path) {
                Ok(()) => worker::extract_path(path, &self.shared.cancel),
                Err(e) => Err(extract::ExtractError::new(ErrorKind::UnsafeFile, &e)),
            };
            let after = fs::symlink_metadata(path).ok();
            if !matches!((&before, &after), (Some(a), Some(b)) if a.len() == b.len() && filesystem::stamp(a) == filesystem::stamp(b) && !filesystem::unsafe_attributes(b))
            {
                result = Err(extract::ExtractError::new(
                    ErrorKind::Changed,
                    "文件仍在变化，稍后复核",
                ));
            }
            if result.is_ok() {
                verified = after;
            }
            if !matches!(&result, Err(e) if e.kind == ErrorKind::Changed) {
                break;
            }
            if attempt < 2 {
                thread::sleep(Duration::from_millis(100));
            }
        }
        match result {
            Ok(document) => {
                let after = match verified {
                    Some(meta) => meta,
                    _ => {
                        self.store.remove(&path_string);
                        self.known.remove(&id);
                        self.pending += 1;
                        report(
                            self.shared,
                            path,
                            "提取后文件已变化或不可访问，未保存该内容",
                        );
                        return Ok(true);
                    }
                };
                self.bytes += document.text.len();
                self.store.put(
                    &path_string,
                    after.len(),
                    filesystem::stamp(&after),
                    &document,
                )?;
                self.known.insert(
                    id,
                    FileMeta {
                        path: path_string,
                        size: after.len(),
                        modified: filesystem::stamp(&after),
                    },
                );
                self.pending += 1;
            }
            Err(e) if e.kind == ErrorKind::Cancelled => return Ok(false),
            Err(e) => {
                self.store.remove(&path_string);
                self.known.remove(&id);
                self.pending += 1;
                report(self.shared, path, e.message);
            }
        }
        if self.pending >= COMMIT_FILES
            || self.bytes >= 2 * 1024 * 1024
            || self.last_flush.elapsed() >= Duration::from_secs(2)
        {
            self.flush()?;
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "manual Release indexing throughput measurement"]
    fn measure_initial_index_throughput() {
        let temp =
            tempfile::tempdir_in(Path::new(env!("CARGO_MANIFEST_DIR")).join("target")).unwrap();
        let documents = temp.path().join("documents");
        fs::create_dir(&documents).unwrap();
        let body =
            "合同签署、项目验收、会议记录。 ASCII content AND % _，跨格式检索。\n".repeat(16);
        let files = 10_000;
        for number in 0..files {
            fs::write(documents.join(format!("{number}.txt")), &body).unwrap();
        }
        let start = Instant::now();
        let mut scope = Scope::new(vec![documents], vec![]);
        scope.extensions = vec!["txt".into()];
        let engine = Engine::start(scope, temp.path().join("index"));
        engine.refresh(false).unwrap();
        let mut first_commit = None;
        loop {
            let status = engine.status();
            if status.indexed > 0 && first_commit.is_none() {
                first_commit = Some(start.elapsed().as_secs_f64());
            }
            if status.phase == "idle" && status.indexed == files {
                assert!(status.complete);
                break;
            }
            assert!(status.phase != "error", "{}", status.message);
            assert!(start.elapsed() < Duration::from_secs(120));
            thread::sleep(Duration::from_millis(10));
        }
        let elapsed = start.elapsed().as_secs_f64();
        engine.stop();
        println!(
            "{}",
            serde_json::json!({"files": files, "source_bytes": files * body.len() as u64, "seconds": elapsed, "files_per_second": files as f64 / elapsed, "first_commit_seconds": first_commit})
        );
    }

    #[test]
    fn status_poll_releases_idle_query_without_waiting_for_the_indexer() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(&temp.path().join("search-index")).unwrap();
        let engine = Engine::start(
            Scope::new(vec![temp.path().into()], vec![]),
            temp.path().join("unused-index"),
        );
        let session = Session::new(&store.reader, 1, "合同").unwrap();
        engine.shared.search.lock().unwrap().session = Some(session);
        engine.status();
        assert!(engine.shared.search.lock().unwrap().session.is_some());
        engine
            .shared
            .search
            .lock()
            .unwrap()
            .session
            .as_mut()
            .unwrap()
            .touched = Instant::now() - Duration::from_secs(181);
        let busy = engine.shared.search.lock().unwrap();
        engine.status();
        assert!(busy.session.is_some());
        drop(busy);
        engine.status();
        assert!(engine.shared.search.lock().unwrap().session.is_none());
        engine.stop();
    }

    #[test]
    fn overflow_error_and_unknown_notification_request_full_reconciliation() {
        let temp = tempfile::tempdir().unwrap();
        let scope = Scope::new(vec![temp.path().into()], vec![]);
        let engine = Engine::start(scope.clone(), temp.path().join("index"));
        let (sender, _receiver) = mpsc::sync_channel(1);
        let event = || {
            notify::Event::new(notify::EventKind::Modify(notify::event::ModifyKind::Any))
                .add_path(temp.path().join("test.txt"))
        };
        queue_event(&engine.shared, &scope, &sender, Ok(event()));
        assert!(!engine.shared.dirty.load(Ordering::Acquire));
        queue_event(&engine.shared, &scope, &sender, Ok(event()));
        assert!(engine.shared.dirty.swap(false, Ordering::AcqRel));
        queue_event(
            &engine.shared,
            &scope,
            &sender,
            Err(notify::Error::generic("synthetic watcher failure")),
        );
        assert!(engine.shared.dirty.swap(false, Ordering::AcqRel));
        queue_event(
            &engine.shared,
            &scope,
            &sender,
            Ok(notify::Event::new(notify::EventKind::Any)),
        );
        assert!(engine.shared.dirty.load(Ordering::Acquire));
        engine.stop();
    }
}
