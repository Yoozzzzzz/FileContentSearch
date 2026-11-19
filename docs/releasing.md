# GitHub Release 发布指南

[返回 README](../README.md)

当前版本为 `0.1.0`，适合发布 Windows x64 独立 EXE，或只包含该 EXE 的 ZIP。`src-tauri/tauri.conf.json` 中的 `bundle.active` 为 `false`，现有构建不会生成安装向导，也不会为用户安装 WebView2。

以下步骤由维护者在已准备依赖的 Windows x64 电脑上执行；所有命令均从项目根目录运行。本指南不代表已经创建了 GitHub Release。

## 1. 确定版本和源代码

首次发布使用 `v0.1.0`。后续版本同步修改 `package.json`、`src-tauri/tauri.conf.json` 和 `src-tauri/Cargo.toml` 中的项目版本，并更新前端及 Cargo 锁文件中的项目版本，提交并推送后再构建。

先确认工作区没有待发布却未提交的修改，记录将要发布的提交：

```powershell
git status --short
git rev-parse HEAD
```

在打标签前保持同一提交，避免上传的 EXE 与标签对应源码不一致。不要移动已经发布的版本标签；需要修正时发布新版本。

## 2. 检查与构建

运行 [README 中的验证命令](../README.md#验证)，全部成功后构建。先正常关闭占用旧 EXE 的应用窗口，再执行：

```powershell
. ./scripts/dev-env.ps1
npm.cmd ci
if ($LASTEXITCODE -ne 0) { throw '前端依赖安装失败' }
npm.cmd run tauri -- build --no-bundle -- --locked
if ($LASTEXITCODE -ne 0) { throw 'Release 构建失败，停止发布' }
```

`--no-bundle` 跳过安装包阶段，`-- --locked` 将锁文件约束传给 Cargo。构建命令会同时生成前端资源与 Rust Release 程序；CLI 参数已对照本项目安装的 Tauri CLI 2.12.6 帮助与[官方 CLI 文档](https://v2.tauri.app/reference/cli/#build)核对。

默认 x64 MSVC 工具链且未覆盖 Cargo 输出目录时，产物为：

```text
src-tauri\target\release\file-content-search.exe
```

手动运行刚构建的程序，使用合成数据检查搜索、高亮、类型筛选、退出与重启：

```powershell
.\src-tauri\target\release\file-content-search.exe --demo
```

正常数据目录与生产启动行为按[既定验收范围](design.md#验证重点与交付边界)复核；不要让测试脚本自行启动全盘索引。记录实际完成的检查，不把历史性能数值写成本次重测结果。

## 3. 准备附件

可以直接上传 EXE。为了让文件名包含版本和平台，推荐打成 ZIP，并生成 SHA-256 校验文件。以下以 `v0.1.0` 为例；重发时使用新的版本名，不覆盖已经发布的附件。

```powershell
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force .\.local\releases | Out-Null
$asset = '.\.local\releases\FileContentSearch-v0.1.0-windows-x64.zip'
Compress-Archive -LiteralPath .\src-tauri\target\release\file-content-search.exe -DestinationPath $asset
$hash = (Get-FileHash -LiteralPath $asset -Algorithm SHA256).Hash.ToLowerInvariant()
"$hash  $(Split-Path -Leaf $asset)" | Set-Content -LiteralPath "$asset.sha256" -Encoding ascii
```

上传以下两个附件：

```text
FileContentSearch-v0.1.0-windows-x64.zip
FileContentSearch-v0.1.0-windows-x64.zip.sha256
```

`.local/` 已被 Git 忽略。ZIP 只包含程序，不包含索引、`types.json`、私人文档、日志、WebView2 缓存、`node_modules` 或整个 `target` 目录。发布前解压该 ZIP 并检查其中的 EXE，确认压缩后的交付物也能运行。

## 4. 创建标签并上传 GitHub

在构建所用的提交上创建标签并推送：

```powershell
git tag -a v0.1.0 -m 'FileContentSearch v0.1.0'
if ($LASTEXITCODE -ne 0) { throw '创建标签失败，请检查是否已存在' }
git push origin main
if ($LASTEXITCODE -ne 0) { throw '推送 main 失败' }
git push origin v0.1.0
if ($LASTEXITCODE -ne 0) { throw '推送标签失败' }
```

然后在网页操作：

1. 打开仓库的 [新建 Release 页面](https://github.com/Yoozzzzzz/FileContentSearch/releases/new)，或从 **Releases → Draft a new release** 进入。
2. 在 **Choose a tag** 中选择刚推送的 `v0.1.0`。
3. 标题填写 `FileContentSearch v0.1.0`，说明可参考下方模板。
4. 将 ZIP 与 `.sha256` 文件拖入附件区域，等待上传完成。
5. 先 **Save draft** 核对标签、附件和说明；首次面向其他电脑试用时可选 **This is a pre-release**。确认后点击 **Publish release**。

GitHub 支持在 Release 中附加编译产物；源码 ZIP/tar.gz 由平台生成，不能替代可运行程序。网页操作依据 [GitHub 官方发布说明](https://docs.github.com/en/repositories/releasing-projects-on-github/managing-releases-in-a-repository)。

## Release 说明模板

按实际完成的检查调整以下内容：

```markdown
Windows x64 本机文档内容检索工具，首个版本。

功能：
- DOCX 正文及表格、文字版 PDF、Markdown 和可选纯文本检索。
- 连续字面匹配、高亮片段、目录树与结果类型筛选。
- 启动复核、运行期自动更新，以及确认后移入回收站。

使用：
- 下载 Windows x64 ZIP，解压后运行 file-content-search.exe。
- 需要 Microsoft Edge WebView2，索引固定保存在 D:\FileContentSearchData\。
- 默认索引 DOCX / PDF / MD / Markdown；首次启动需确认建立索引。
- 索引包含原文副本，默认未加密；文档处理在本机完成。

限制：
- JSON、旧 .doc、Excel/PPT 和扫描件 OCR 暂不支持。
- 单文件上限 32MiB；复杂 PDF 的字体映射和阅读顺序可能受限。
- 当前验证范围为开发者本机与合成样本，100GB 规模尚未验证。

本次验证：填写本次实际通过的检查及测试环境。
```

## 需要安装向导时

当前 EXE/ZIP 发布不需要修改应用配置。如果后续需要 NSIS 安装向导或 MSI，可启用 `bundle.active`、选择打包目标并按 [Tauri Windows 安装包文档](https://v2.tauri.app/distribute/windows-installer/)构建。安装包可配置 WebView2 安装方式；这条路径尚未在本项目验收，不应把当前独立 EXE 叫作安装包。
