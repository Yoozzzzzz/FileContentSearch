use crate::extract::{Document, Location, LocationKind};
use serde::Serialize;
use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
use tantivy::{
    DocAddress, Index, IndexReader, IndexWriter, ReloadPolicy, Searcher, TERMINATED,
    TantivyDocument, Term,
    directory::MmapDirectory,
    query::{BooleanQuery, EnableScoring, Occur, Query, Scorer, TermQuery},
    schema::*,
    tokenizer::NgramTokenizer,
};

pub type Result<T> = std::result::Result<T, String>;
fn error(e: impl std::fmt::Display) -> String {
    format!("索引操作失败：{e}")
}
pub fn key(path: &str) -> String {
    let mut units: Vec<u16> = path.replace('/', "\\").encode_utf16().collect();
    if !units.is_empty() {
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::CharUpperBuffW(
                units.as_mut_ptr(),
                units.len() as u32,
            );
        }
    }
    String::from_utf16_lossy(&units).to_ascii_lowercase()
}

#[derive(Clone)]
struct Fields {
    id: Field,
    path: Field,
    size: Field,
    modified: Field,
    text: Field,
    grams: Field,
    locations: Field,
    kind: Field,
}
fn schema() -> (Schema, Fields) {
    let mut b = Schema::builder();
    let id = b.add_text_field("id", STRING);
    let path = b.add_text_field("path", TextOptions::default().set_fast("raw").set_stored());
    let size = b.add_u64_field("size", FAST | STORED);
    let modified = b.add_u64_field("modified", FAST | STORED);
    let text = b.add_text_field("text", STORED);
    let grams = b.add_text_field(
        "grams",
        TextOptions::default().set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer("characters")
                .set_index_option(IndexRecordOption::Basic)
                .set_fieldnorms(false),
        ),
    );
    let locations = b.add_text_field("locations", STORED);
    let kind = b.add_text_field("kind", STORED);
    (
        b.build(),
        Fields {
            id,
            path,
            size,
            modified,
            text,
            grams,
            locations,
            kind,
        },
    )
}

pub struct Store {
    pub index: Index,
    pub reader: IndexReader,
    pub writer: IndexWriter,
    pub last_updated: u64,
    fields: Fields,
}
impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        std::fs::create_dir_all(path).map_err(error)?;
        let (schema, fields) = schema();
        let index = Index::open_or_create(MmapDirectory::open(path).map_err(error)?, schema)
            .map_err(error)?;
        index.tokenizers().register(
            "characters",
            NgramTokenizer::new(1, 2, false).map_err(error)?,
        );
        let writer = index
            .writer_with_num_threads(1, 32 * 1024 * 1024)
            .map_err(error)?;
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()
            .map_err(error)?;
        let last_updated = index
            .load_metas()
            .map_err(error)?
            .payload
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        Ok(Self {
            index,
            reader,
            writer,
            fields,
            last_updated,
        })
    }
    pub fn remove(&self, path: &str) {
        self.writer
            .delete_term(Term::from_field_text(self.fields.id, &key(path)));
    }
    pub fn put(&self, path: &str, size: u64, modified: u64, document: &Document) -> Result<()> {
        let f = &self.fields;
        let mut doc = TantivyDocument::default();
        doc.add_text(f.id, key(path));
        doc.add_text(f.path, path);
        doc.add_u64(f.size, size);
        doc.add_u64(f.modified, modified);
        doc.add_text(f.text, &document.text);
        doc.add_text(f.grams, document.text.to_ascii_lowercase());
        doc.add_text(
            f.locations,
            serde_json::to_string(&document.locations).map_err(error)?,
        );
        doc.add_text(
            f.kind,
            serde_json::to_string(&document.location_kind).map_err(error)?,
        );
        self.remove(path);
        self.writer.add_document(doc).map_err(error)?;
        Ok(())
    }
    pub fn commit(&mut self) -> Result<()> {
        let now = crate::filesystem::now();
        let result = (|| {
            let mut commit = self.writer.prepare_commit()?;
            commit.set_payload(&now.to_string());
            commit.commit()
        })();
        if let Err(e) = result {
            let _ = self.writer.rollback();
            return Err(error(e));
        }
        self.reader.reload().map_err(error)?;
        self.last_updated = now;
        Ok(())
    }
    pub fn rollback(&mut self) -> Result<()> {
        self.writer.rollback().map(|_| ()).map_err(error)
    }
    pub fn metadata(&self) -> Result<HashMap<String, FileMeta>> {
        metadata(&self.reader.searcher())
    }
}

#[derive(Clone, Debug)]
pub struct FileMeta {
    pub path: String,
    pub size: u64,
    pub modified: u64,
}
fn metadata(searcher: &Searcher) -> Result<HashMap<String, FileMeta>> {
    let mut files = HashMap::new();
    visit_metadata(searcher, |file| {
        files.insert(key(&file.path), file);
    })?;
    Ok(files)
}

#[derive(Default, Serialize)]
pub struct Statistics {
    pub total: u64,
    pub source_bytes: u64,
    pub text: u64,
    pub docx: u64,
    pub pdf: u64,
}
pub fn statistics(searcher: &Searcher) -> Result<Statistics> {
    let mut stats = Statistics::default();
    visit_metadata(searcher, |file| {
        stats.total += 1;
        stats.source_bytes += file.size;
        match Path::new(&file.path)
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "docx" => stats.docx += 1,
            "pdf" => stats.pdf += 1,
            _ => stats.text += 1,
        }
    })?;
    Ok(stats)
}

fn visit_metadata(searcher: &Searcher, mut visit: impl FnMut(FileMeta)) -> Result<()> {
    for segment in searcher.segment_readers() {
        let paths = segment
            .fast_fields()
            .str("path")
            .map_err(error)?
            .ok_or("索引路径字段缺失")?;
        let sizes = segment.fast_fields().u64("size").map_err(error)?;
        let modified = segment.fast_fields().u64("modified").map_err(error)?;
        for doc in 0..segment.max_doc() {
            if segment.is_deleted(doc) {
                continue;
            }
            let mut path = String::new();
            if let Some(ord) = paths.term_ords(doc).next() {
                paths.ord_to_str(ord, &mut path).map_err(error)?;
            }
            visit(FileMeta {
                path,
                size: sizes.first(doc).unwrap_or(0),
                modified: modified.first(doc).unwrap_or(0),
            });
        }
    }
    Ok(())
}

#[derive(Serialize)]
pub struct Hit {
    pub id: usize,
    pub path: String,
    pub name: String,
    pub size: u64,
}
#[derive(Serialize)]
pub struct Page {
    pub request: u64,
    pub offset: usize,
    pub hits: Vec<Hit>,
    pub found: usize,
    pub complete: bool,
    pub engine_ms: f64,
    pub read_ms: f64,
    pub verify_ms: f64,
}
#[derive(Serialize, Clone, Debug)]
pub struct Part {
    pub text: String,
    pub matched: bool,
}
#[derive(Serialize, Clone, Debug)]
pub struct Snippet {
    pub location: u32,
    pub kind: LocationKind,
    pub parts: Vec<Part>,
}
#[derive(Serialize)]
pub struct Preview {
    pub request: u64,
    pub id: usize,
    pub snippets: Vec<Snippet>,
}

pub struct Session {
    pub request: u64,
    searcher: Searcher,
    fields: Fields,
    query: String,
    extensions: Option<Vec<String>>,
    paths: Vec<tantivy::columnar::StrColumn>,
    scorers: Vec<Box<dyn Scorer>>,
    segment: usize,
    emitted: Vec<DocAddress>,
    cache: VecDeque<(usize, Arc<CachedDocument>)>,
    cache_bytes: usize,
    setup_ms: f64,
    pub touched: Instant,
}
struct CachedDocument {
    doc: TantivyDocument,
    normalized: String,
    bytes: usize,
}
impl Session {
    pub fn new(reader: &IndexReader, request: u64, input: &str) -> Result<Self> {
        Self::filtered(reader, request, input, None)
    }
    pub fn filtered(
        reader: &IndexReader,
        request: u64,
        input: &str,
        extensions: Option<Vec<String>>,
    ) -> Result<Self> {
        if let Some(ref types) = extensions
            && !types.is_empty()
        {
            crate::extract::validate_extensions(types)?;
        }
        let started = Instant::now();
        if input.chars().count() > 256 {
            return Err("关键词最多 256 个字符".into());
        }
        let (_, fields) = schema();
        let searcher = reader.searcher();
        let query = input.to_ascii_lowercase();
        let chars: Vec<char> = query.chars().collect();
        let terms: BTreeSet<String> = if chars.len() == 1 {
            [query.clone()].into_iter().collect()
        } else {
            chars.windows(2).map(|pair| pair.iter().collect()).collect()
        };
        let mut scorers = Vec::new();
        let mut paths = Vec::new();
        if !terms.is_empty() {
            let clauses = terms
                .into_iter()
                .map(|t| {
                    (
                        Occur::Must,
                        Box::new(TermQuery::new(
                            Term::from_field_text(fields.grams, &t),
                            IndexRecordOption::Basic,
                        )) as Box<dyn Query>,
                    )
                })
                .collect();
            let weight = BooleanQuery::new(clauses)
                .weight(EnableScoring::disabled_from_searcher(&searcher))
                .map_err(error)?;
            for segment in searcher.segment_readers() {
                scorers.push(weight.scorer(segment, 1.0).map_err(error)?);
                paths.push(
                    segment
                        .fast_fields()
                        .str("path")
                        .map_err(error)?
                        .ok_or("索引路径字段缺失")?,
                );
            }
        }
        Ok(Self {
            request,
            searcher,
            fields,
            query,
            extensions,
            paths,
            scorers,
            segment: 0,
            emitted: Vec::new(),
            cache: VecDeque::new(),
            cache_bytes: 0,
            setup_ms: started.elapsed().as_secs_f64() * 1000.0,
            touched: Instant::now(),
        })
    }
    pub fn page(&mut self, offset: usize, limit: usize) -> Result<Page> {
        if limit == 0 || limit > 50 || offset != self.emitted.len() {
            return Err("无效分页游标或批次大小".into());
        }
        self.touched = Instant::now();
        let mut read_ms = 0.0;
        let mut verify_ms = 0.0;
        let mut hits = Vec::new();
        while self.segment < self.scorers.len() && hits.len() < limit {
            let scorer = &mut self.scorers[self.segment];
            let doc_id = scorer.doc();
            if doc_id == TERMINATED {
                self.segment += 1;
                continue;
            }
            scorer.advance();
            let segment = self.searcher.segment_reader(self.segment as u32);
            if segment.is_deleted(doc_id) {
                continue;
            }
            if let Some(types) = &self.extensions {
                let column = &self.paths[self.segment];
                let mut path = String::new();
                if let Some(ord) = column.term_ords(doc_id).next() {
                    column.ord_to_str(ord, &mut path).map_err(error)?;
                }
                if !crate::extract::matches_extensions(Path::new(&path), types) {
                    if self.touched.elapsed() >= Duration::from_millis(25) {
                        break;
                    }
                    continue;
                }
            }
            let address = DocAddress::new(self.segment as u32, doc_id);
            let reading = Instant::now();
            let doc: TantivyDocument = self.searcher.doc(address).map_err(error)?;
            read_ms += reading.elapsed().as_secs_f64() * 1000.0;
            let verifying = Instant::now();
            let normalized = string(&doc, self.fields.text)?.to_ascii_lowercase();
            let matched = normalized.contains(&self.query);
            verify_ms += verifying.elapsed().as_secs_f64() * 1000.0;
            if matched {
                let path = string(&doc, self.fields.path)?.to_owned();
                let name = Path::new(&path)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                hits.push(Hit {
                    id: self.emitted.len(),
                    path,
                    name,
                    size: doc
                        .get_first(self.fields.size)
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                });
                self.remember(self.emitted.len(), doc, normalized)?;
                self.emitted.push(address);
            }
            // A budget is an unfinished page, never a claim of complete results.
            if self.touched.elapsed() >= Duration::from_millis(25) {
                break;
            }
        }
        while self.segment < self.scorers.len() && self.scorers[self.segment].doc() == TERMINATED {
            self.segment += 1;
        }
        Ok(Page {
            request: self.request,
            offset,
            hits,
            found: self.emitted.len(),
            complete: self.segment == self.scorers.len(),
            engine_ms: std::mem::take(&mut self.setup_ms)
                + self.touched.elapsed().as_secs_f64() * 1000.0,
            read_ms,
            verify_ms,
        })
    }
    fn remember(
        &mut self,
        id: usize,
        doc: TantivyDocument,
        normalized: String,
    ) -> Result<Arc<CachedDocument>> {
        const BUDGET: usize = 64 * 1024 * 1024;
        let bytes = normalized.len() * 2
            + string(&doc, self.fields.locations)?.len()
            + string(&doc, self.fields.path)?.len()
            + 512;
        let cached = Arc::new(CachedDocument {
            doc,
            normalized,
            bytes,
        });
        if bytes <= BUDGET {
            while self.cache_bytes + bytes > BUDGET || self.cache.len() >= 128 {
                if let Some((_, old)) = self.cache.pop_front() {
                    self.cache_bytes -= old.bytes;
                }
            }
            self.cache_bytes += bytes;
            self.cache.push_back((id, cached.clone()));
        }
        debug_assert!(self.cache_bytes <= BUDGET && self.cache.len() <= 128);
        Ok(cached)
    }
    fn document(&mut self, id: usize) -> Result<Arc<CachedDocument>> {
        self.touched = Instant::now();
        if let Some((_, doc)) = self.cache.iter().find(|(key, _)| *key == id) {
            return Ok(doc.clone());
        }
        let address = *self.emitted.get(id).ok_or("文件不属于当前查询结果")?;
        let doc = self.searcher.doc(address).map_err(error)?;
        let normalized = string(&doc, self.fields.text)?.to_ascii_lowercase();
        self.remember(id, doc, normalized)
    }
    pub fn path(&mut self, id: usize) -> Result<String> {
        let doc = self.document(id)?;
        Ok(string(&doc.doc, self.fields.path)?.into())
    }
    pub fn file_meta(&mut self, id: usize) -> Result<FileMeta> {
        let doc = self.document(id)?;
        Ok(FileMeta {
            path: string(&doc.doc, self.fields.path)?.into(),
            size: doc
                .doc
                .get_first(self.fields.size)
                .and_then(|v| v.as_u64())
                .ok_or("缺少文件大小")?,
            modified: doc
                .doc
                .get_first(self.fields.modified)
                .and_then(|v| v.as_u64())
                .ok_or("缺少文件版本")?,
        })
    }
    pub fn preview(&mut self, id: usize, limit: usize) -> Result<Preview> {
        if limit == 0 || limit > 8 {
            return Err("预览批次超限".into());
        }
        let doc = self.document(id)?;
        let text = string(&doc.doc, self.fields.text)?;
        let locations: Vec<Location> =
            serde_json::from_str(string(&doc.doc, self.fields.locations)?).map_err(error)?;
        let kind = serde_json::from_str(string(&doc.doc, self.fields.kind)?).map_err(error)?;
        Ok(Preview {
            request: self.request,
            id,
            snippets: normalized_snippets(
                text,
                &doc.normalized,
                &self.query,
                kind,
                &locations,
                limit,
            ),
        })
    }
}
fn string(doc: &TantivyDocument, field: Field) -> Result<&str> {
    doc.get_first(field)
        .and_then(|v| v.as_str())
        .ok_or_else(|| "索引存储字段缺失".into())
}
pub fn snippets(
    text: &str,
    query: &str,
    kind: LocationKind,
    locations: &[Location],
    limit: usize,
) -> Vec<Snippet> {
    normalized_snippets(
        text,
        &text.to_ascii_lowercase(),
        query,
        kind,
        locations,
        limit,
    )
}
fn normalized_snippets(
    text: &str,
    normalized: &str,
    query: &str,
    kind: LocationKind,
    locations: &[Location],
    limit: usize,
) -> Vec<Snippet> {
    if query.is_empty() {
        return Vec::new();
    }
    let mut snippets = Vec::new();
    let mut after = 0;
    while snippets.len() < limit {
        let Some(relative) = normalized[after..].find(query) else {
            break;
        };
        let start = after + relative;
        let end = start + query.len();
        let left = text[..start]
            .char_indices()
            .rev()
            .nth(79)
            .map_or(0, |(i, _)| i);
        let right = text[end..]
            .char_indices()
            .nth(80)
            .map_or(text.len(), |(i, _)| end + i);
        let mut parts = Vec::new();
        let mut position = left;
        for (relative, _) in normalized[left..right].match_indices(query) {
            let hit = left + relative;
            if hit > position {
                parts.push(Part {
                    text: text[position..hit].into(),
                    matched: false,
                });
            }
            parts.push(Part {
                text: text[hit..hit + query.len()].into(),
                matched: true,
            });
            position = hit + query.len();
        }
        if position < right {
            parts.push(Part {
                text: text[position..right].into(),
                matched: false,
            });
        }
        let location = locations
            .iter()
            .rfind(|l| l.start <= start)
            .map_or(0, |l| l.number);
        snippets.push(Snippet {
            location,
            kind,
            parts,
        });
        after = position;
    }
    snippets
}
