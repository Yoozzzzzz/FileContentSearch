pub mod engine;
pub mod extract;
pub mod filesystem;
pub mod index;
pub mod recycle;
pub mod worker;

use engine::Engine;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tauri::Manager;

#[tauri::command]
async fn engine_status(engine: tauri::State<'_, Arc<Engine>>) -> Result<engine::Status, String> {
    let engine = engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || engine.status())
        .await
        .map_err(|_| "状态读取任务异常".into())
}
#[tauri::command]
async fn index_statistics(
    engine: tauri::State<'_, Arc<Engine>>,
) -> Result<index::Statistics, String> {
    let engine = engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || engine.statistics())
        .await
        .map_err(|_| "索引统计任务异常")?
}
#[tauri::command]
async fn refresh_index(engine: tauri::State<'_, Arc<Engine>>, rebuild: bool) -> Result<(), String> {
    let engine = engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || engine.refresh(rebuild))
        .await
        .map_err(|_| "索引任务提交异常")?
}
#[tauri::command]
fn cancel_index(engine: tauri::State<'_, Arc<Engine>>) {
    engine.cancel();
}
#[tauri::command]
async fn search(
    engine: tauri::State<'_, Arc<Engine>>,
    request: u64,
    query: String,
    extensions: Option<Vec<String>>,
) -> Result<index::Page, String> {
    let engine = engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        engine.search_filtered(request, &query, extensions)
    })
    .await
    .map_err(|_| "查询任务异常")?
}
#[tauri::command]
async fn set_index_types(
    engine: tauri::State<'_, Arc<Engine>>,
    extensions: Vec<String>,
) -> Result<(), String> {
    let engine = engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || engine.set_extensions(extensions))
        .await
        .map_err(|_| "类型更新任务异常")?
}
#[tauri::command]
async fn recycle_selected(
    engine: tauri::State<'_, Arc<Engine>>,
    request: u64,
    id: usize,
) -> Result<(), String> {
    let engine = engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || engine.recycle_selected(request, id))
        .await
        .map_err(|_| "回收站操作异常")?
}
#[tauri::command]
async fn next_page(
    engine: tauri::State<'_, Arc<Engine>>,
    request: u64,
    offset: usize,
) -> Result<index::Page, String> {
    let engine = engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || engine.page(request, offset))
        .await
        .map_err(|_| "分页任务异常")?
}
#[tauri::command]
async fn previews(
    engine: tauri::State<'_, Arc<Engine>>,
    request: u64,
    ids: Vec<usize>,
    detail: bool,
) -> Result<Vec<index::Preview>, String> {
    let engine = engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || engine.previews(request, &ids, detail))
        .await
        .map_err(|_| "预览任务异常")?
}
#[tauri::command]
fn selected_path(
    engine: tauri::State<'_, Arc<Engine>>,
    request: u64,
    id: usize,
) -> Result<String, String> {
    Ok(engine
        .selected_path(request, id)?
        .to_string_lossy()
        .into_owned())
}
#[tauri::command]
fn open_selected(
    engine: tauri::State<'_, Arc<Engine>>,
    request: u64,
    id: usize,
    folder: bool,
) -> Result<(), String> {
    let path = engine.selected_path(request, id)?;
    let extension = path
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !folder && !matches!(extension.as_str(), "pdf" | "docx") {
        // Script and code extensions must open as text, never run through their associations.
        let windows = std::env::var_os("WINDIR").ok_or("找不到 Windows 文本查看器")?;
        std::process::Command::new(PathBuf::from(windows).join("System32/notepad.exe"))
            .arg(&path)
            .spawn()
            .map_err(|_| "无法打开文本查看器")?;
        return Ok(());
    }
    let target = if folder {
        path.parent().ok_or("文件没有父目录")?
    } else {
        &path
    };
    let verb = filesystem::wide(Path::new("open"));
    let result = unsafe {
        windows_sys::Win32::UI::Shell::ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            filesystem::wide(target).as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
        )
    };
    if result as isize <= 32 {
        return Err("无法打开文件，请检查文件是否存在及默认打开程序".into());
    }
    Ok(())
}

fn demo() -> Result<(filesystem::Scope, PathBuf), String> {
    use std::io::Write;
    let root = std::env::temp_dir().join(format!(
        "FileContentSearch-demo-{}-{}",
        std::process::id(),
        filesystem::now()
    ));
    std::fs::create_dir(&root).map_err(|_| "无法创建独立演示目录")?;
    let docs = root.join("documents");
    std::fs::create_dir(&docs).map_err(|_| "无法创建合成样本目录")?;
    for i in 0..10000 {
        let category = [
            "合同签署",
            "研发计划",
            "项目验收",
            "会议记录",
            "采购申请",
            "测试报告",
            "知识管理",
            "财务报表",
        ][i % 8];
        let text = format!(
            "这是第 {i} 份合成文档，仅用于本机测试。\n{category}需要按照约定执行。\n所有文档包含公共检索词：内容搜索。\nEnglish AND 100% _ \"quoted\" 😀𠮷 重复哈哈哈。\n"
        );
        std::fs::write(docs.join(format!("记录-{i:05}.txt")), text)
            .map_err(|_| "合成样本创建失败")?;
    }
    std::fs::write(
        docs.join("正文表格.docx"),
        include_bytes!("../tests/fixtures/chinese.docx"),
    )
    .map_err(|_| "DOCX 样本创建失败")?;
    std::fs::write(
        docs.join("中文两页.pdf"),
        include_bytes!("../tests/fixtures/chinese.pdf"),
    )
    .map_err(|_| "PDF 样本创建失败")?;
    let mut large =
        std::fs::File::create(docs.join("large-source.txt")).map_err(|_| "大文本样本创建失败")?;
    large
        .write_all("大文件合同验收\n".as_bytes())
        .map_err(|_| "大文本样本写入失败")?;
    let mut block = vec![b'x'; 65536];
    block[65535] = b'\n';
    for _ in 0..480 {
        large.write_all(&block).map_err(|_| "大文本样本写入失败")?;
    }
    drop(large);
    let data = root.join("index-data");
    let mut scope = filesystem::Scope::new(vec![docs], vec![data.clone()]);
    // The fixed benchmark corpus deliberately includes TXT and other supported formats.
    scope.extensions = extract::EXTENSIONS.iter().map(|s| (*s).into()).collect();
    Ok((scope, data))
}
pub fn run() {
    let is_demo = std::env::args_os().any(|a| a == "--demo");
    let (scope, data) = if is_demo {
        demo().expect("无法准备合成演示数据")
    } else {
        (
            filesystem::Scope::production(),
            PathBuf::from(r"D:\FileContentSearchData"),
        )
    };
    let engine = Arc::new(Engine::start(scope, data));
    if is_demo {
        let _ = engine.refresh(false);
    }
    let app = tauri::Builder::default()
        .manage(engine.clone())
        .setup(|app| {
            let local = std::env::var_os("LOCALAPPDATA")
                .ok_or("LOCALAPPDATA 不可用，无法创建本机界面数据目录")?;
            tauri::WebviewWindowBuilder::from_config(app, &app.config().app.windows[0])?
                .data_directory(PathBuf::from(local).join("FileContentSearch/WebView2"))
                .build()?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            engine_status,
            index_statistics,
            refresh_index,
            cancel_index,
            search,
            next_page,
            previews,
            selected_path,
            set_index_types,
            recycle_selected,
            open_selected
        ])
        .build(tauri::generate_context!());
    let app = match app {
        Ok(app) => app,
        Err(error) => {
            engine.stop();
            let message = format!(
                "无法启动桌面窗口。请检查 Microsoft Edge WebView2 运行时是否已安装。\n\n详细信息：{error}"
            );
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
                    std::ptr::null_mut(),
                    filesystem::wide(Path::new(&message)).as_ptr(),
                    filesystem::wide(Path::new("FileContentSearch 启动失败")).as_ptr(),
                    0x10,
                );
            }
            return;
        }
    };
    app.run(|app, event| {
        if let tauri::RunEvent::ExitRequested {
            code: None, api, ..
        } = event
        {
            api.prevent_exit();
            let engine = app.state::<Arc<Engine>>().inner().clone();
            engine.request_stop();
            let handle = app.clone();
            std::thread::spawn(move || {
                engine.stop();
                handle.exit(0);
            });
        }
    });
}
