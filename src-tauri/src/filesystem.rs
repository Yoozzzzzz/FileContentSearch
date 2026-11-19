use crate::{
    extract::supported_extension,
    index::{Result, key},
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    os::windows::{ffi::OsStrExt, fs::MetadataExt},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::Storage::FileSystem::{
    GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, MOVEFILE_REPLACE_EXISTING,
    MOVEFILE_WRITE_THROUGH, MoveFileExW,
};

const OWNER: &str = "FileContentSearch owned index directory v1\n";
pub fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
pub fn stamp(meta: &fs::Metadata) -> u64 {
    meta.last_write_time()
}
pub fn unsafe_attributes(meta: &fs::Metadata) -> bool {
    meta.file_attributes() & (0x400 | 0x1000 | 0x40000 | 0x400000) != 0
}

#[derive(Clone)]
pub struct Scope {
    pub extensions: Vec<String>,
    pub roots: Vec<PathBuf>,
    pub exclusions: Vec<PathBuf>,
    root_keys: Vec<PathBuf>,
    exclusion_keys: Vec<PathBuf>,
    own_temporary: Option<PathBuf>,
}
impl Scope {
    pub fn new(roots: Vec<PathBuf>, exclusions: Vec<PathBuf>) -> Self {
        let normalize = |paths: &[PathBuf]| {
            paths
                .iter()
                .map(|p| PathBuf::from(key(&p.to_string_lossy())))
                .collect()
        };
        Self {
            extensions: crate::extract::DEFAULT_EXTENSIONS
                .iter()
                .map(|s| (*s).into())
                .collect(),
            root_keys: normalize(&roots),
            exclusion_keys: normalize(&exclusions),
            roots,
            exclusions,
            own_temporary: None,
        }
    }
    pub fn production() -> Self {
        let mask = unsafe { GetLogicalDrives() };
        let roots = (0..26)
            .filter(|i| mask & (1 << i) != 0)
            .map(|i| PathBuf::from(format!("{}:\\", (b'A' + i) as char)))
            .filter(|p| unsafe { GetDriveTypeW(wide(p).as_ptr()) } == 3)
            .collect();
        let mut exclusions = vec![PathBuf::from(r"D:\FileContentSearchData")];
        for name in [
            "WINDIR",
            "ProgramFiles",
            "ProgramFiles(x86)",
            "ProgramW6432",
        ] {
            if let Some(p) = std::env::var_os(name) {
                exclusions.push(PathBuf::from(p));
            }
        }
        if let Some(p) = std::env::var_os("LOCALAPPDATA") {
            exclusions.push(PathBuf::from(&p).join("FileContentSearch"));
            exclusions.push(PathBuf::from(p).join("local.filecontentsearch.app"));
        }
        if let Some(p) = std::env::var_os("WEBVIEW2_USER_DATA_FOLDER") {
            exclusions.push(PathBuf::from(p));
        }
        let build_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("project directory");
        for directory in [".tools", ".local", "src-tauri/target", "dist"] {
            exclusions.push(build_root.join(directory));
        }
        if let Some(p) = option_env!("CARGO_HOME") {
            exclusions.push(PathBuf::from(p));
        }
        if let Some(p) = option_env!("RUSTUP_HOME") {
            exclusions.push(PathBuf::from(p));
        }
        exclusions.extend(installed_directories());
        exclusions.sort();
        exclusions.dedup();
        let mut scope = Self::new(roots, exclusions);
        scope.own_temporary = Some(PathBuf::from(key(&std::env::temp_dir().to_string_lossy())));
        scope
    }
    pub fn includes(&self, path: &Path) -> bool {
        let path = PathBuf::from(key(&path.to_string_lossy()));
        self.root_keys.iter().any(|root| path.starts_with(root)) && !self.excluded_key(&path)
    }
    pub fn excluded(&self, path: &Path) -> bool {
        self.excluded_key(&PathBuf::from(key(&path.to_string_lossy())))
    }
    pub fn root_available(&self, path: &Path) -> bool {
        let path = PathBuf::from(key(&path.to_string_lossy()));
        self.root_keys.iter().any(|root| path.starts_with(root))
    }
    fn excluded_key(&self, path: &Path) -> bool {
        let own_demo = self
            .own_temporary
            .as_ref()
            .and_then(|root| path.strip_prefix(root).ok())
            .and_then(|relative| relative.components().next())
            .and_then(|part| part.as_os_str().to_str())
            .and_then(|name| name.strip_prefix("filecontentsearch-demo-"))
            .is_some_and(|suffix| {
                !suffix.is_empty() && suffix.bytes().all(|c| c.is_ascii_digit() || c == b'-')
            });
        let installation_root = path.components().nth(2).is_some_and(|part| {
            matches!(
                part.as_os_str().to_str(),
                Some("windows" | "program files" | "program files (x86)")
            )
        });
        own_demo
            || installation_root
            || self
                .exclusion_keys
                .iter()
                .any(|root| path.starts_with(root))
            || path.components().any(|c| {
                matches!(
                    c.as_os_str().to_str(),
                    Some(".git" | "node_modules" | "$recycle.bin" | "system volume information")
                )
            })
    }
    pub fn validate_file(&self, path: &Path) -> Result<()> {
        if !path.is_absolute()
            || path
                .components()
                .any(|p| matches!(p, std::path::Component::ParentDir))
            || !self.includes(path)
        {
            return Err("文件不在允许扫描的范围".into());
        }
        for ancestor in path.ancestors() {
            let meta = fs::symlink_metadata(ancestor).map_err(|_| "文件或父目录不可访问")?;
            if unsafe_attributes(&meta) {
                return Err("不读取链接、联接点或云端占位文件".into());
            }
        }
        let extension = path
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !supported_extension(&extension) {
            return Err("不支持此文件类型".into());
        }
        Ok(())
    }
}

fn installed_directories() -> Vec<PathBuf> {
    use windows_sys::Win32::System::Registry::*;
    let mut paths = Vec::new();
    let protected: Vec<_> = ["USERPROFILE", "LOCALAPPDATA", "APPDATA", "PUBLIC"]
        .into_iter()
        .filter_map(std::env::var_os)
        .map(|p| PathBuf::from(key(&PathBuf::from(p).to_string_lossy())))
        .collect();
    for root in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        for view in [KEY_WOW64_64KEY, KEY_WOW64_32KEY] {
            let mut handle = std::ptr::null_mut();
            if unsafe {
                RegOpenKeyExW(
                    root,
                    wide(Path::new(
                        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
                    ))
                    .as_ptr(),
                    0,
                    KEY_READ | view,
                    &mut handle,
                )
            } != 0
            {
                continue;
            }
            for index in 0..10000 {
                let mut name = [0u16; 256];
                let mut length = name.len() as u32;
                let result = unsafe {
                    RegEnumKeyExW(
                        handle,
                        index,
                        name.as_mut_ptr(),
                        &mut length,
                        std::ptr::null(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    )
                };
                if result != 0 {
                    break;
                }
                let mut value = vec![0u16; 32768];
                let mut bytes = (value.len() * 2) as u32;
                if unsafe {
                    RegGetValueW(
                        handle,
                        name.as_ptr(),
                        wide(Path::new("InstallLocation")).as_ptr(),
                        RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ,
                        std::ptr::null_mut(),
                        value.as_mut_ptr().cast(),
                        &mut bytes,
                    )
                } == 0
                {
                    let end = value.iter().position(|&c| c == 0).unwrap_or(value.len());
                    if let Ok(value) = String::from_utf16(&value[..end]) {
                        let path = PathBuf::from(value.trim().trim_matches('"'));
                        let normalized = PathBuf::from(key(&path.to_string_lossy()));
                        // Bad installer metadata must not exclude a whole drive or user data root.
                        if path.is_absolute()
                            && path.components().count() >= 3
                            && !protected.iter().any(|p| {
                                p.starts_with(&normalized)
                                    || (normalized.parent() == Some(p.as_path()))
                            })
                        {
                            paths.push(path);
                        }
                    }
                }
            }
            unsafe {
                RegCloseKey(handle);
            }
        }
    }
    paths
}

#[derive(Serialize, Deserialize)]
struct Active {
    format: u32,
    generation: String,
}
pub struct DataDirectory {
    pub root: PathBuf,
}
impl DataDirectory {
    pub fn extensions(&self, defaults: &[String]) -> Result<Vec<String>> {
        let path = self.root.join("types.json");
        let metadata = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(defaults.to_vec());
            }
            Err(_) => return Err("无法读取文件类型配置".into()),
        };
        if !metadata.is_file() || unsafe_attributes(&metadata) || metadata.len() > 4096 {
            return Err("文件类型配置超限或为链接".into());
        }
        let extensions = serde_json::from_slice::<Vec<String>>(
            &fs::read(path).map_err(|_| "无法读取文件类型配置")?,
        )
        .map_err(|_| "文件类型配置损坏")?;
        crate::extract::validate_extensions(&extensions)?;
        Ok(extensions)
    }
    pub fn save_extensions(&self, extensions: &[String]) -> Result<()> {
        crate::extract::validate_extensions(extensions)?;
        self.check_space()?;
        let temporary = self.root.join("types.next");
        if let Ok(meta) = fs::symlink_metadata(&temporary) {
            if !meta.is_file() || unsafe_attributes(&meta) {
                return Err("类型暂存配置不安全".into());
            }
            fs::remove_file(&temporary).map_err(|_| "无法清理类型暂存配置")?;
        }
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|_| "无法保存文件类型配置")?;
        file.write_all(&serde_json::to_vec(extensions).map_err(|_| "类型配置编码失败")?)
            .and_then(|_| file.sync_all())
            .map_err(|_| "无法刷新文件类型配置")?;
        drop(file);
        if unsafe {
            MoveFileExW(
                wide(&temporary).as_ptr(),
                wide(&self.root.join("types.json")).as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err("文件类型配置保存失败".into());
        }
        Ok(())
    }
    pub fn open(root: &Path, create: bool) -> Result<Self> {
        if !root.is_absolute() {
            return Err("索引目录必须是绝对路径".into());
        }
        let parent = root.parent().ok_or("无效索引目录")?;
        if !parent.is_dir() {
            return Err("索引磁盘不可用；未迁移到其他磁盘".into());
        }
        for ancestor in parent.ancestors() {
            if unsafe_attributes(&fs::symlink_metadata(ancestor).map_err(|_| "索引目录不可访问")?)
            {
                return Err("索引目录不能经过链接或占位符".into());
            }
        }
        if !root.exists() {
            if !create {
                return Err("尚未建立索引".into());
            }
            fs::create_dir(root).map_err(|_| "无法创建专用索引目录")?;
        }
        if unsafe_attributes(&fs::symlink_metadata(root).map_err(|_| "索引目录不可访问")?) {
            return Err("索引目录不能是链接".into());
        }
        let marker = root.join(".owner");
        if !marker.exists()
            && create
            && fs::read_dir(root)
                .map_err(|_| "无法检查索引目录")?
                .next()
                .is_none()
        {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&marker)
                .map_err(|_| "无法创建索引目录标记")?;
            file.write_all(OWNER.as_bytes())
                .and_then(|_| file.sync_all())
                .map_err(|_| "无法保存索引目录标记")?;
        }
        let marker_metadata = fs::symlink_metadata(&marker)
            .map_err(|_| "目标目录没有有效的应用标记，未覆盖该目录")?;
        if !marker_metadata.is_file()
            || unsafe_attributes(&marker_metadata)
            || marker_metadata.len() > 4096
            || fs::read_to_string(&marker).ok().as_deref() != Some(OWNER)
        {
            return Err("目标目录含有其他数据或标记无效，未覆盖该目录".into());
        }
        let directory = Self {
            root: root.to_path_buf(),
        };
        Ok(directory)
    }
    pub fn check_space(&self) -> Result<()> {
        let mut available = 0;
        if unsafe {
            GetDiskFreeSpaceExW(
                wide(&self.root).as_ptr(),
                &mut available,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err("无法获取索引磁盘剩余空间".into());
        }
        if available < 1024 * 1024 * 1024 {
            return Err("索引磁盘剩余空间不足 1GiB，已暂停".into());
        }
        Ok(())
    }
    pub fn active(&self) -> Result<Option<PathBuf>> {
        let path = self.root.join("active.json");
        if !path.exists() {
            return Ok(None);
        }
        let meta = fs::symlink_metadata(&path).map_err(|_| "无法读取索引配置")?;
        if unsafe_attributes(&meta) || !meta.is_file() || meta.len() > 4096 {
            return Err("索引配置超限或为链接".into());
        }
        let active: Active =
            serde_json::from_slice(&fs::read(path).map_err(|_| "无法读取索引配置")?)
                .map_err(|_| "索引配置损坏")?;
        if active.format != 1 {
            return Err("索引格式不兼容".into());
        }
        let path = self.generation(&active.generation)?;
        if !path.is_dir()
            || unsafe_attributes(&fs::symlink_metadata(&path).map_err(|_| "索引目录不存在")?)
        {
            return Err("索引代目录不可用".into());
        }
        Ok(Some(path))
    }
    fn generation(&self, name: &str) -> Result<PathBuf> {
        if !name.starts_with("index-")
            || name.len() > 80
            || !name
                .bytes()
                .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase() || c == b'-')
        {
            return Err("无效的索引代目录".into());
        }
        Ok(self.root.join(name))
    }
    pub fn new_generation(&self) -> Result<PathBuf> {
        self.check_space()?;
        let name = format!(
            "index-{}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            std::process::id()
        );
        let path = self.generation(&name)?;
        fs::create_dir(&path).map_err(|_| "无法创建重建目录")?;
        Ok(path)
    }
    pub fn activate(&self, path: &Path) -> Result<()> {
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or("无效索引名称")?;
        if self.generation(name)? != path {
            return Err("索引切换越界".into());
        }
        let data = serde_json::to_vec(&Active {
            format: 1,
            generation: name.into(),
        })
        .map_err(|_| "索引配置编码失败")?;
        let temporary = self.root.join("active.next");
        if temporary.exists() {
            if unsafe_attributes(&fs::symlink_metadata(&temporary).map_err(|_| "无法检查暂存配置")?)
            {
                return Err("暂存配置不能是链接".into());
            }
            fs::remove_file(&temporary).map_err(|_| "无法清理上次未完成的配置")?;
        }
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|_| "无法保存索引配置")?;
        file.write_all(&data)
            .and_then(|_| file.sync_all())
            .map_err(|_| "无法刷新索引配置")?;
        drop(file);
        if unsafe {
            MoveFileExW(
                wide(&temporary).as_ptr(),
                wide(&self.root.join("active.json")).as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err("索引切换失败；保留上次已发布索引".into());
        }
        Ok(())
    }
    pub fn remove_generation(&self, path: &Path) -> Result<()> {
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or("无效清理目录")?;
        if self.generation(name)? != path || self.active()?.as_deref() == Some(path) {
            return Err("拒绝清理活动索引或目录之外的数据".into());
        }
        let root = self.root.canonicalize().map_err(|_| "无法验证索引目录")?;
        let target = path.canonicalize().map_err(|_| "无法验证清理目录")?;
        if target.parent() != Some(root.as_path())
            || unsafe_attributes(&fs::symlink_metadata(path).map_err(|_| "清理目录不可访问")?)
        {
            return Err("拒绝越界清理".into());
        }
        fs::remove_dir_all(target).map_err(|_| "旧索引暂无法清理，请稍后重试".into())
    }

    pub fn cleanup_inactive(&self) -> Vec<String> {
        let active = match self.active() {
            Ok(active) => active,
            Err(e) => return vec![e],
        };
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(_) => return vec!["无法检查旧索引目录".into()],
        };
        let mut errors = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if active.as_deref() == Some(path.as_path()) {
                continue;
            }
            if entry
                .file_name()
                .to_str()
                .is_some_and(|name| self.generation(name).is_ok())
                && let Err(e) = self.remove_generation(&path)
            {
                errors.push(e);
            }
        }
        errors
    }
}
