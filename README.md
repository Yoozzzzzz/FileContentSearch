# FileContentSearch

Windows 本机文档内容检索工具。记得正文中的一句话，就能找到文件，并查看高亮片段及其行号、段落或页码。

基于 **Tauri 2 · Rust · Tantivy · TanStack Virtual**，文档解析、索引和搜索全部在本机完成。

[下载与版本](https://github.com/Yoozzzzzz/FileContentSearch/releases) · [开发构建](#开发构建) · [发布指南](docs/releasing.md) · [验证记录](docs/verification.md)

## 功能

- **按内容搜索**：支持中文连续子串和英文 ASCII 大小写不敏感匹配，预览中高亮命中内容。
- **文档与纯文本**：提取 DOCX 正文及表格、文字版 PDF、Markdown、TXT 和支持的代码文本。
- **目录树结果**：按完整磁盘与目录层级展示，支持折叠、类型筛选、键盘导航和虚拟滚动。
- **自动更新**：启动时复核遗漏，运行期间监听文件变化；支持手动刷新、取消和重建，退出后不驻留。
- **文件操作**：打开文件、打开所在目录、复制路径，以及确认后移入 Windows 回收站。
- **本机处理**：无需账号、模型 API 或云端检索服务，不上传文档正文，不启用遥测。

首版功能已实现，当前版本为 **0.1.0**。已完成本机 Release 与合成样本验证；100GB 是后续目标规模，尚未完成该规模验收。

## 快速开始

### 运行条件

| 项目 | 要求 |
| --- | --- |
| 平台 | Windows x64；当前验证范围为开发者本机 |
| 界面运行时 | Microsoft Edge WebView2；缺失时先安装 [Evergreen Runtime](https://developer.microsoft.com/en-us/microsoft-edge/webview2/#download-section) |
| 数据磁盘 | 可用的 D 盘，索引固定保存在 `D:\FileContentSearchData\` |
| 文档 | 本机固定磁盘上的受支持文件，使用当前用户权限读取 |

运行已编译的程序不需要安装 Node.js、Rust、Python 或 Microsoft Office。当前发布方式为独立 EXE 或包含 EXE 的 ZIP，WebView2 需由系统提供；数据仍保存在上述固定位置。

### 下载和使用

1. 打开 [Releases](https://github.com/Yoozzzzzz/FileContentSearch/releases)，下载发布者上传的 Windows x64 程序附件；若还没有附件，请按下方步骤从源码构建。GitHub 自动生成的 `Source code` 是源码包。
2. 若下载的是 ZIP，先解压，再运行 `file-content-search.exe`。
3. 首次启动确认扫描范围，点击「建立本机索引」。默认索引类型仅为 **DOCX / PDF / MD / Markdown**，可在「索引类型」中调整。
4. 输入正文关键词，在左侧选择文件，在右侧查看命中片段。首次索引过程中可搜索已提交的部分。

已保存的类型选择优先于默认值，重启后保留。取消某种类型会清理其索引记录，原文件保留。D 盘不可用或空间不足时暂停，不自动迁移到 C 盘。同一份生产索引只支持一个运行实例。

## 支持的文件

| 格式 | 提取内容 | 默认索引 |
| --- | --- | --- |
| `.docx` | 正文段落与表格；位置为段落号，不是 Word 页码 | 是 |
| `.pdf` | 可提取的文字与页码；复杂字体和排版可能影响结果 | 是 |
| `.md` / `.markdown` | 纯文本与行号 | 是 |
| `.txt` 及下列文本类型 | 纯文本与行号；需在「索引类型」中启用 | 否 |

可选文本扩展名：`txt csv log xml html css rs ts tsx js jsx toml yaml yml py java c h cpp hpp cs go sql ps1 sh`。扩展名不区分 ASCII 大小写；文本编码支持 UTF-8、带 BOM 的 UTF-16，解码失败会报告原因。

不支持 JSON、旧 `.doc`、Excel/PPT、图片或扫描版 PDF 的 OCR、压缩包内部搜索。DOCX 页眉页脚、批注及嵌入对象不在当前范围；预览只展示提取文本，不还原原版排版。

## 搜索与操作

输入按**连续字面文本**匹配：`合同签署` 必须连续出现；空格、引号、`AND`、`%`、`_` 都是普通字符。不提供正则、通配符、多关键词语法、繁简转换或语义搜索。空输入不执行搜索。

| 操作 | 行为 |
| --- | --- |
| 索引类型 | 决定哪些扩展名进入索引，至少选择一种；「恢复默认」选择四种默认类型 |
| 结果文件类型 | 仅筛选当前搜索，不改变索引设置 |
| 目录折叠 / 展开 | 目录数字是当前已加载的命中文件数；带 `+` 表示搜索尚未完成 |
| ↑ / ↓、← / → | 选择节点、折叠 / 展开目录或在父子节点间导航 |
| Enter | 在搜索框中重新查询；在结果树中打开选中文件 |
| 刷新 / 取消 / 重建 | 复核变化、停止任务、建立新索引；重建完整成功后才切换，期间保留可用旧索引 |
| 移入回收站 | 选中文件后再次确认，只回收该文件；无法回收时报错，不降级为永久删除 |

「覆盖尚未完整」表示扫描尚未结束或有目录无法完整访问，可在「索引详情」查看原因。损坏、加密、无文本、权限不足及超限文件会单独报告，不中止整个批次。索引任务空闲后自动更新当前查询；查询闲置三分钟后释放快照，继续操作时需重新搜索。

## 数据位置与边界

| 数据 | 位置 |
| --- | --- |
| 索引、重建暂存及类型选择 `types.json` | `D:\FileContentSearchData\` |
| WebView2 配置与缓存 | `%LOCALAPPDATA%\FileContentSearch\WebView2\` |
| 演示样本与测试索引 | 系统临时目录中的 `FileContentSearch-demo-*` |

扫描本机固定磁盘，排除系统目录、程序安装目录、回收站、`.git`、`node_modules` 和应用数据目录；不跟随目录链接或联接点，不主动下载云文件占位符，不主动提权。

索引包含原文副本，**默认未加密**，不要作为 Release 附件或提交到仓库。检索与预览只读源文件；只有用户确认的「移入回收站」会移动文件。回收站可能由系统或用户清理。

单个源文件上限 **32MiB**，提取文本上限 **64MiB**。PDF 在同一 EXE 的临时子进程内解析，最多一个任务，限制为 512MiB 提交内存和 60 秒；超限报告失败，不把截断文本当作完整索引。更多边界见[验证记录](docs/verification.md)。

## 开发构建

需要 Node.js **24.14.1 至 25 之前的版本**、Rust **1.100.0 / MSVC**、Microsoft C++ Build Tools（C++ 桌面开发及 Windows SDK）和 WebView2。准备步骤见 [Tauri 官方环境要求](https://v2.tauri.app/start/prerequisites/)。

以下命令在 Windows PowerShell 中执行：

```powershell
git clone https://github.com/Yoozzzzzz/FileContentSearch.git
cd FileContentSearch
. ./scripts/dev-env.ps1
npm.cmd ci
npm.cmd run tauri -- dev
```

`dev-env.ps1` 仅在项目内存在 `.tools/` 工具链时为当前会话配置路径；全新克隆需要自行安装 Rust 和 C++ 构建工具。Rust 版本由 `rust-toolchain.toml` 固定，依赖由 `package-lock.json` 与 `src-tauri/Cargo.lock` 锁定。依赖准备需要网络，实际文档处理使用本地资源。

构建可运行的 Release EXE：

```powershell
. ./scripts/dev-env.ps1
npm.cmd run tauri -- build --no-bundle -- --locked
```

产物为 `src-tauri\target\release\file-content-search.exe`。当前 `bundle.active` 为 `false`，不会生成 MSI 或安装向导；上传版本号、ZIP 与校验文件的步骤见[GitHub Release 发布指南](docs/releasing.md)。

## 验证

在已准备依赖的项目根目录运行：

```powershell
. ./scripts/dev-env.ps1
npm.cmd run build
node scripts/tree-check.mjs
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --locked --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --locked
```

自动化测试只使用临时数据和合成样本，不启动全盘索引。32 项常规 Rust 测试以及前端构建、目录树断言、格式与 Clippy 检查已通过。

需要手动体验合成文档时，先构建 Release，再运行：

```powershell
.\src-tauri\target\release\file-content-search.exe --demo
```

演示模式只扫描其独立临时目录，创建 10,000 份小文本、一份约 30MiB 正文和两个 DOCX/PDF 样本，共 10,003 份文档。它启用全部支持类型，不改变生产类型选择，也不创建生产索引。样本重建及可选吞吐测试见[验证记录](docs/verification.md)。

### 已测表现与限制

以下是本机 Release 的历史记录，测试条件与完整数据见[验证记录](docs/verification.md)：

| 场景 | 已观察到的结果 |
| --- | --- |
| 合成集普通查询首屏 | P50 33.2ms / P95 33.4ms，包含输入提交、IPC、可见摘要及绘制 |
| 单份约 30MiB 文本查询首屏 | P50 83.3ms / P95 99.7ms |
| 历史 218,764 文件快照（5.706GiB） | 普通查询首屏 P50 50.3ms / P95 83.4ms |
| 10,000 条结果滚动 | 文件行 DOM 最多 15 个；120 帧间隔 P95 16.8ms，无 long task |
| 历史暂停状态整组进程内存 | 私有工作集约 345–355MiB，包含 WebView2，未达到“几十 MB”的挑战目标 |

真实快照的性能与内存数据采集于排除 JSON 和目录树调整之前，不代表当前版本重测结果。**100GB 语料、完整首次索引时长、复杂真实 PDF 峰值和系统冷缓存仍未验证**；物理断网验收也未完成，已有记录仅为 WebView 离线模拟。

## 项目结构与文档

```text
src/                       # 原生 TypeScript、CSS、目录树与搜索界面
src-tauri/src/             # Rust 提取、索引、扫描、PDF 子进程与文件操作
src-tauri/tests/           # 集成测试及合成样本
scripts/                   # 开发环境、样本生成、交互和性能检查
docs/                      # 发布指南、设计约定及验证记录
```

- [发布指南](docs/releasing.md)：本机构建、打包、标签及 GitHub 附件发布。
- [设计与验收约定](docs/design.md)：产品范围、技术选择、实施路线、模块和验收标准。
- [验证记录](docs/verification.md)：历史测试、测量口径、PDF 边界与已知限制。
- [AI 协作指南](AGENTS.md)：修改规则、隐私边界和验证要求。
