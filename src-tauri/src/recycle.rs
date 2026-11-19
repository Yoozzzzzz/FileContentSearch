use crate::{
    filesystem::{self, Scope},
    index::{FileMeta, Result},
};
use std::{
    fs,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use windows::{
    Win32::{Foundation::E_ABORT, System::Com::*, UI::Shell::*},
    core::{HRESULT, PCWSTR, Ref, implement},
};

fn validate(scope: &Scope, file: &FileMeta) -> Result<()> {
    let path = Path::new(&file.path);
    scope.validate_file(path)?;
    let meta = fs::symlink_metadata(path).map_err(|_| "文件已不存在或无法访问")?;
    if !meta.is_file() || meta.len() != file.size || filesystem::stamp(&meta) != file.modified {
        return Err("文件已变化，请刷新结果后重新确认删除".into());
    }
    Ok(())
}

#[implement(IFileOperationProgressSink)]
struct RecycleSink {
    scope: Scope,
    file: FileMeta,
    recycled: Arc<AtomicBool>,
}

#[allow(non_snake_case, unused_variables)]
impl IFileOperationProgressSink_Impl for RecycleSink_Impl {
    fn PreDeleteItem(&self, flags: u32, item: Ref<IShellItem>) -> windows::core::Result<()> {
        // Windows retries permanent deletion without this flag: reject that callback.
        if flags & TSF_DELETE_RECYCLE_IF_POSSIBLE.0 as u32 == 0
            || validate(&self.scope, &self.file).is_err()
        {
            return Err(E_ABORT.into());
        }
        Ok(())
    }
    fn PostDeleteItem(
        &self,
        flags: u32,
        item: Ref<IShellItem>,
        result: HRESULT,
        recycled: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        result.ok()?;
        if recycled.is_none() {
            return Err(E_ABORT.into());
        }
        self.recycled.store(true, Ordering::Release);
        Ok(())
    }
    fn StartOperations(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn FinishOperations(&self, hrresult: windows::core::HRESULT) -> windows::core::Result<()> {
        Ok(())
    }
    fn PreRenameItem(
        &self,
        dwflags: u32,
        psiitem: windows::core::Ref<IShellItem>,
        psznewname: &windows::core::PCWSTR,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostRenameItem(
        &self,
        dwflags: u32,
        psiitem: windows::core::Ref<IShellItem>,
        psznewname: &windows::core::PCWSTR,
        hrrename: windows::core::HRESULT,
        psinewlycreated: windows::core::Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PreMoveItem(
        &self,
        dwflags: u32,
        psiitem: windows::core::Ref<IShellItem>,
        psidestinationfolder: windows::core::Ref<IShellItem>,
        psznewname: &windows::core::PCWSTR,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostMoveItem(
        &self,
        dwflags: u32,
        psiitem: windows::core::Ref<IShellItem>,
        psidestinationfolder: windows::core::Ref<IShellItem>,
        psznewname: &windows::core::PCWSTR,
        hrmove: windows::core::HRESULT,
        psinewlycreated: windows::core::Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PreCopyItem(
        &self,
        dwflags: u32,
        psiitem: windows::core::Ref<IShellItem>,
        psidestinationfolder: windows::core::Ref<IShellItem>,
        psznewname: &windows::core::PCWSTR,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostCopyItem(
        &self,
        dwflags: u32,
        psiitem: windows::core::Ref<IShellItem>,
        psidestinationfolder: windows::core::Ref<IShellItem>,
        psznewname: &windows::core::PCWSTR,
        hrcopy: windows::core::HRESULT,
        psinewlycreated: windows::core::Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PreNewItem(
        &self,
        dwflags: u32,
        psidestinationfolder: windows::core::Ref<IShellItem>,
        psznewname: &windows::core::PCWSTR,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostNewItem(
        &self,
        dwflags: u32,
        psidestinationfolder: windows::core::Ref<IShellItem>,
        psznewname: &windows::core::PCWSTR,
        psztemplatename: &windows::core::PCWSTR,
        dwfileattributes: u32,
        hrnew: windows::core::HRESULT,
        psinewitem: windows::core::Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn UpdateProgress(&self, iworktotal: u32, iworksofar: u32) -> windows::core::Result<()> {
        Ok(())
    }
    fn ResetTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn PauseTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn ResumeTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
}

pub fn recycle(scope: &Scope, file: &FileMeta) -> Result<()> {
    validate(scope, file)?;
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
        .ok()
        .map_err(|_| "无法初始化 Windows 回收站")?;
    let result = (|| -> windows::core::Result<()> {
        let recycled = Arc::new(AtomicBool::new(false));
        let sink: IFileOperationProgressSink = RecycleSink {
            scope: scope.clone(),
            file: file.clone(),
            recycled: recycled.clone(),
        }
        .into();
        unsafe {
            let operation: IFileOperation =
                CoCreateInstance(&FileOperation, None, CLSCTX_INPROC_SERVER)?;
            operation.SetOperationFlags(
                FOFX_RECYCLEONDELETE
                    | FOFX_ADDUNDORECORD
                    | FOFX_EARLYFAILURE
                    | FOF_NOERRORUI
                    | FOF_SILENT
                    | FOF_NOCONFIRMATION
                    | FOF_NO_CONNECTED_ELEMENTS,
            )?;
            let path = filesystem::wide(Path::new(&file.path));
            let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(path.as_ptr()), None)?;
            operation.DeleteItem(&item, &sink)?;
            operation.PerformOperations()?;
            if operation.GetAnyOperationsAborted()?.as_bool() || !recycled.load(Ordering::Acquire) {
                return Err(E_ABORT.into());
            }
        }
        Ok(())
    })();
    unsafe { CoUninitialize() };
    result.map_err(|error| {
        format!(
            "无法移入回收站（Windows 错误 {}）；未请求永久删除",
            error.code()
        )
    })
}
