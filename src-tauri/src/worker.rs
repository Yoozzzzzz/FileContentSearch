use crate::extract::{self, Document, ErrorKind, ExtractError, Limits, Location, LocationKind};
use std::{
    io::{Read, Write},
    os::windows::{
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::CommandExt,
    },
    path::Path,
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use windows_sys::Win32::System::{JobObjects::*, Threading::CREATE_NO_WINDOW};

const MEMORY_LIMIT: usize = 512 * 1024 * 1024;
const TIME_LIMIT: Duration = Duration::from_secs(60);
static PDF_WARNING: AtomicBool = AtomicBool::new(false);
struct PdfLogger;
impl log::Log for PdfLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Warn
    }
    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            PDF_WARNING.store(true, Ordering::Relaxed);
        }
    }
    fn flush(&self) {}
}

fn failure(kind: ErrorKind, text: &str) -> ExtractError {
    ExtractError::new(kind, text)
}

pub fn extract_path(path: &Path, cancelled: &AtomicBool) -> extract::Result<Document> {
    let limits = Limits::default();
    let (bytes, extension) = extract::read_file(path, limits)?;
    if cancelled.load(Ordering::Relaxed) {
        return Err(failure(ErrorKind::Cancelled, "提取已取消"));
    }
    if extension == "pdf" {
        let executable = std::env::current_exe().map_err(ExtractError::from)?;
        extract_pdf(&executable, bytes, cancelled)
    } else {
        extract::extract_bytes(&bytes, &extension, limits)
    }
}

pub fn extract_pdf(
    executable: &Path,
    bytes: Vec<u8>,
    cancelled: &AtomicBool,
) -> extract::Result<Document> {
    if bytes.len() > Limits::default().source_bytes {
        return Err(failure(ErrorKind::Limit, "源文件超过大小上限"));
    }
    let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if handle.is_null() {
        return Err(failure(ErrorKind::Io, "无法创建解析资源限制"));
    }
    let job = unsafe { OwnedHandle::from_raw_handle(handle) };
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
        ProcessMemoryLimit: MEMORY_LIMIT,
        ..Default::default()
    };
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_PROCESS_MEMORY
        | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        | JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
    limits.BasicLimitInformation.ActiveProcessLimit = 1;
    let configured = unsafe {
        SetInformationJobObject(
            job.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            size_of_val(&limits) as u32,
        )
    };
    if configured == 0 {
        return Err(failure(ErrorKind::Io, "无法设置解析资源限制"));
    }
    let mut child = Command::new(executable)
        .arg("--extract-worker")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(ExtractError::from)?;
    // The child reads no input until its job is attached, so parser allocations are always limited.
    if unsafe { AssignProcessToJobObject(job.as_raw_handle(), child.as_raw_handle()) } == 0 {
        let _ = child.kill();
        let _ = child.wait();
        return Err(failure(ErrorKind::Io, "无法约束解析进程，已停止该任务"));
    }
    let mut stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let input = std::thread::spawn(move || -> std::io::Result<()> {
        stdin.write_all(&(bytes.len() as u32).to_le_bytes())?;
        stdin.write_all(&bytes)
    });
    let output = std::thread::spawn(move || read_result(stdout));
    let started = Instant::now();
    let outcome = loop {
        if cancelled.load(Ordering::Relaxed) {
            break Err(failure(ErrorKind::Cancelled, "提取已取消"));
        }
        if started.elapsed() >= TIME_LIMIT {
            break Err(failure(
                ErrorKind::Timeout,
                "PDF 解析超过 60 秒，已结束该文件任务",
            ));
        }
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break Ok(()),
            Ok(Some(_)) => {
                break Err(failure(
                    ErrorKind::Corrupt,
                    "PDF 解析进程异常结束，可能超出内存预算或文档损坏",
                ));
            }
            Err(_) => break Err(failure(ErrorKind::Io, "无法获取解析任务状态")),
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
        }
    };
    drop(job);
    let _ = child.wait();
    let written = input
        .join()
        .map_err(|_| failure(ErrorKind::Io, "解析输入任务异常"))?;
    let result = output
        .join()
        .map_err(|_| failure(ErrorKind::Io, "解析输出任务异常"))?;
    outcome?;
    written.map_err(ExtractError::from)?;
    result
}

fn read_u32(reader: &mut impl Read) -> std::io::Result<u32> {
    let mut bytes = [0u8; 4];
    reader.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_result(mut reader: impl Read) -> extract::Result<Document> {
    let mut status = [0u8];
    reader.read_exact(&mut status)?;
    if status[0] == 0 {
        let size = read_u32(&mut reader)? as usize;
        if size > 4096 {
            return Err(failure(ErrorKind::Io, "解析错误响应超限"));
        }
        let mut bytes = vec![0u8; size];
        reader.read_exact(&mut bytes)?;
        return Err(serde_json::from_slice(&bytes)
            .map_err(|_| failure(ErrorKind::Io, "解析错误响应无效"))?);
    }
    if status[0] != 1 {
        return Err(failure(ErrorKind::Io, "解析协议无效"));
    }
    let size = read_u32(&mut reader)? as usize;
    if size > Limits::default().text_bytes {
        return Err(failure(ErrorKind::Limit, "解析结果超过上限"));
    }
    let mut bytes = vec![0u8; size];
    reader.read_exact(&mut bytes)?;
    let text =
        String::from_utf8(bytes).map_err(|_| failure(ErrorKind::Decode, "解析输出编码错误"))?;
    let count = read_u32(&mut reader)? as usize;
    if count > Limits::default().pdf_pages {
        return Err(failure(ErrorKind::Limit, "解析页码超过上限"));
    }
    let mut locations = Vec::with_capacity(count);
    let mut previous = 0;
    for index in 0..count {
        let start = read_u32(&mut reader)? as usize;
        if start < previous || !text.is_char_boundary(start) {
            return Err(failure(ErrorKind::Corrupt, "解析位置无效"));
        }
        previous = start;
        locations.push(Location {
            start,
            number: index as u32 + 1,
        });
    }
    Ok(Document {
        text,
        location_kind: LocationKind::Page,
        locations,
    })
}

pub fn run_child() {
    std::panic::set_hook(Box::new(|_| {}));
    unsafe {
        windows_sys::Win32::System::Diagnostics::Debug::SetErrorMode(0x0001 | 0x0002 | 0x8000);
    }
    static LOGGER: PdfLogger = PdfLogger;
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(log::LevelFilter::Warn);
    let result = (|| {
        let mut input = std::io::stdin().lock();
        let size = read_u32(&mut input)? as usize;
        if size > Limits::default().source_bytes {
            return Err(failure(ErrorKind::Limit, "源文件超过上限"));
        }
        let mut bytes = vec![0u8; size];
        input.read_exact(&mut bytes)?;
        let document = extract::extract_bytes(&bytes, "pdf", Limits::default())?;
        if PDF_WARNING.load(Ordering::Relaxed) {
            return Err(failure(
                ErrorKind::Corrupt,
                "PDF 解析出现兼容或解码警告，无法保证提取完整，未加入有效索引",
            ));
        }
        Ok(document)
    })();
    let mut output = std::io::stdout().lock();
    let written = (|| -> std::io::Result<()> {
        match result {
            Ok(document) => {
                output.write_all(&[1])?;
                output.write_all(&(document.text.len() as u32).to_le_bytes())?;
                output.write_all(document.text.as_bytes())?;
                output.write_all(&(document.locations.len() as u32).to_le_bytes())?;
                for location in document.locations {
                    output.write_all(&(location.start as u32).to_le_bytes())?;
                }
            }
            Err(error) => {
                let bytes = serde_json::to_vec(&error).map_err(std::io::Error::other)?;
                output.write_all(&[0])?;
                output.write_all(&(bytes.len() as u32).to_le_bytes())?;
                output.write_all(&bytes)?;
            }
        }
        output.flush()
    })();
    if written.is_err() {
        std::process::exit(1);
    }
}
