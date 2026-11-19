mod docx;
mod pdf;
mod text;

use serde::{Deserialize, Serialize};
use std::{fmt, fs::File, io::Read, path::Path};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Limits {
    pub source_bytes: usize,
    pub text_bytes: usize,
    pub xml_bytes: usize,
    pub zip_entries: usize,
    pub xml_depth: usize,
    pub locations: usize,
    pub pdf_pages: usize,
    pub pdf_decoded_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            source_bytes: 32 * 1024 * 1024,
            text_bytes: 64 * 1024 * 1024,
            xml_bytes: 128 * 1024 * 1024,
            zip_entries: 4096,
            xml_depth: 128,
            locations: 1_000_000,
            pdf_pages: 10_000,
            pdf_decoded_bytes: 128 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    Unsupported,
    PermissionDenied,
    Io,
    Changed,
    UnsafeFile,
    Decode,
    Corrupt,
    Encrypted,
    Limit,
    NoText,
    Cancelled,
    Timeout,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractError {
    pub kind: ErrorKind,
    pub message: String,
}

impl ExtractError {
    pub(crate) fn new(kind: ErrorKind, message: &str) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    fn limit(message: &str) -> Self {
        Self::new(ErrorKind::Limit, message)
    }
    fn corrupt(message: &str) -> Self {
        Self::new(ErrorKind::Corrupt, message)
    }
}

impl fmt::Display for ExtractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for ExtractError {}
impl From<std::io::Error> for ExtractError {
    fn from(error: std::io::Error) -> Self {
        if matches!(error.raw_os_error(), Some(32 | 33)) {
            Self::new(ErrorKind::Changed, "文件正在被写入或占用，需要重试")
        } else if error.kind() == std::io::ErrorKind::PermissionDenied {
            Self::new(ErrorKind::PermissionDenied, "没有读取文件的权限")
        } else {
            Self::new(ErrorKind::Io, "文件读取失败")
        }
    }
}

pub type Result<T> = std::result::Result<T, ExtractError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocationKind {
    Line,
    Paragraph,
    Page,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    /// Internal UTF-8 byte position; never used as a JavaScript string offset.
    pub start: usize,
    pub number: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    pub text: String,
    pub location_kind: LocationKind,
    pub locations: Vec<Location>,
}

impl Document {
    fn new(location_kind: LocationKind) -> Self {
        Self {
            text: String::new(),
            location_kind,
            locations: Vec::new(),
        }
    }

    fn push(&mut self, value: &str, limits: Limits) -> Result<()> {
        if value.len() > limits.text_bytes.saturating_sub(self.text.len()) {
            return Err(ExtractError::limit("提取文本超过字节上限，未保存截断内容"));
        }
        self.text.push_str(value);
        Ok(())
    }

    fn mark(&mut self, limits: Limits) -> Result<()> {
        if self.locations.len() >= limits.locations || self.locations.len() >= u32::MAX as usize {
            return Err(ExtractError::limit("位置记录超过上限"));
        }
        self.locations.push(Location {
            start: self.text.len(),
            number: self.locations.len() as u32 + 1,
        });
        Ok(())
    }
}

pub fn supported_extension(extension: &str) -> bool {
    EXTENSIONS.contains(&extension)
}

pub const EXTENSIONS: &[&str] = &[
    "txt", "docx", "pdf", "md", "markdown", "csv", "log", "xml", "html", "css", "rs", "ts", "tsx",
    "js", "jsx", "toml", "yaml", "yml", "py", "java", "c", "h", "cpp", "hpp", "cs", "go", "sql",
    "ps1", "sh",
];

pub const DEFAULT_EXTENSIONS: &[&str] = &["docx", "pdf", "md", "markdown"];

pub fn validate_extensions(extensions: &[String]) -> std::result::Result<(), String> {
    if extensions.is_empty()
        || extensions.len() > EXTENSIONS.len()
        || extensions.iter().any(|ext| !supported_extension(ext))
        || extensions
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len()
            != extensions.len()
    {
        return Err("请至少选择一种支持的文件类型，不允许重复或未知扩展名".into());
    }
    Ok(())
}

pub fn matches_extensions(path: &Path, extensions: &[String]) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            extensions
                .iter()
                .any(|allowed| ext.eq_ignore_ascii_case(allowed))
        })
}

pub fn extract_bytes(bytes: &[u8], extension: &str, limits: Limits) -> Result<Document> {
    if bytes.len() > limits.source_bytes {
        return Err(ExtractError::limit("源文件超过大小上限"));
    }
    let extension = extension.to_ascii_lowercase();
    if !supported_extension(&extension) {
        return Err(ExtractError::new(
            ErrorKind::Unsupported,
            "暂不支持此文件格式（旧版 .doc 不属于 DOCX）",
        ));
    }
    let document = match extension.as_str() {
        "docx" => docx::extract(bytes, limits)?,
        "pdf" => pdf::extract(bytes, limits)?,
        _ => text::extract(bytes, limits)?,
    };
    if document.text.trim().is_empty() {
        return Err(ExtractError::new(
            ErrorKind::NoText,
            if extension == "pdf" {
                "PDF 没有可提取文本，可能是扫描件，需要 OCR"
            } else {
                "文件没有可提取文本"
            },
        ));
    }
    Ok(document)
}

pub fn extract_file(path: &Path, limits: Limits) -> Result<Document> {
    let (bytes, extension) = read_file(path, limits)?;
    extract_bytes(&bytes, &extension, limits)
}

pub fn read_file(path: &Path, limits: Limits) -> Result<(Vec<u8>, String)> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !supported_extension(&extension) {
        return Err(ExtractError::new(
            ErrorKind::Unsupported,
            "暂不支持此文件格式",
        ));
    }
    // Check before opening to avoid hydrating cloud placeholders.
    let before = std::fs::symlink_metadata(path)?;
    check_file(&before)?;
    if before.len() > limits.source_bytes as u64 {
        return Err(ExtractError::limit("源文件超过大小上限"));
    }
    let mut options = File::options();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000); // FILE_FLAG_OPEN_REPARSE_POINT
    }
    let file = options.open(path)?;
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        let mut buffer = vec![0u16; 32768];
        let length = unsafe {
            windows_sys::Win32::Storage::FileSystem::GetFinalPathNameByHandleW(
                file.as_raw_handle(),
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                0,
            )
        } as usize;
        if length == 0 || length >= buffer.len() {
            return Err(ExtractError::new(
                ErrorKind::UnsafeFile,
                "无法验证文件的最终路径",
            ));
        }
        let resolved = String::from_utf16(&buffer[..length])
            .map_err(|_| ExtractError::new(ErrorKind::UnsafeFile, "文件路径不是有效 Unicode"))?;
        let resolved = resolved.strip_prefix(r"\\?\").unwrap_or(&resolved);
        let absolute = std::path::absolute(path)?;
        let expected = absolute.to_string_lossy();
        let expected = expected.strip_prefix(r"\\?\").unwrap_or(&expected);
        if crate::index::key(resolved) != crate::index::key(expected) {
            return Err(ExtractError::new(
                ErrorKind::UnsafeFile,
                "文件路径经过链接或在打开时变化",
            ));
        }
    }
    let opened = file.metadata()?;
    check_file(&opened)?;
    let mut bytes = Vec::new();
    (&file)
        .take(limits.source_bytes as u64 + 1)
        .read_to_end(&mut bytes)?;
    let after = std::fs::symlink_metadata(path)?;
    if opened.len() != after.len()
        || opened.modified()? != after.modified()?
        || before.len() != opened.len()
        || before.modified()? != opened.modified()?
    {
        return Err(ExtractError::new(
            ErrorKind::Changed,
            "文件在读取期间发生变化，需要重试",
        ));
    }
    check_file(&after)?;
    if bytes.len() > limits.source_bytes {
        return Err(ExtractError::limit("源文件超过大小上限"));
    }
    Ok((bytes, extension))
}

pub(crate) fn check_file(metadata: &std::fs::Metadata) -> Result<()> {
    let mut unsafe_file = !metadata.is_file() || metadata.file_type().is_symlink();
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // Reparse point, offline, recall-on-open and recall-on-data-access.
        unsafe_file |= metadata.file_attributes() & (0x400 | 0x1000 | 0x40000 | 0x400000) != 0;
    }
    if unsafe_file {
        return Err(ExtractError::new(
            ErrorKind::UnsafeFile,
            "跳过链接、云占位符或非普通文件",
        ));
    }
    Ok(())
}
